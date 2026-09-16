//! Platform-independent core of the pre-ConPTY console agent's screen scrape.
//!
//! The Windows console agent (`adapters/windows/console_agent.rs`) reads a
//! screen buffer with `ReadConsoleOutputW` and re-encodes each row into a vt100
//! byte stream. The re-encoding itself touches nothing Windows-specific: it is
//! a walk over decoded cells that emits text and SGR sequences. Keeping it here,
//! off the `#[cfg(windows)]` path, lets it be unit-tested on any host — which is
//! the only place the double-width handling below can be exercised without a
//! real legacy console.
//!
//! ## Double-width (CJK) handling
//!
//! A double-width glyph occupies two console cells. The console is *supposed* to
//! flag the second cell with `COMMON_LVB_TRAILING_BYTE`, and this code still
//! honours that bit. But on legacy consoles (Windows Server 2016 / build 14393,
//! the exact hosts this agent exists for) that bit is unreliable and often
//! unset, so the trailing cell — which carries a space or a duplicate code unit —
//! was being emitted as an extra character. The symptom is a stray space/ASCII
//! between Chinese characters and cumulative column drift on mixed CJK+ASCII
//! lines.
//!
//! The fix makes continuation robust by construction: after emitting a glyph
//! whose *display width* is 2, the following cell is skipped as its
//! continuation regardless of the LVB bit. The width oracle is the same one the
//! downstream vt100 parser uses to advance its cursor
//! (`unicode_width::UnicodeWidthChar`, see `third_party/vt100/src/screen.rs`
//! `Screen::text` and `src/cell.rs` `Cell::set`, both `c.width().unwrap_or(1) >
//! 1`), so "console cells consumed" equals "parser columns advanced" by
//! construction. The LVB bit is retained as a secondary signal so nothing
//! regresses on modern consoles that do set it.

use unicode_width::UnicodeWidthChar;

// CHAR_INFO attribute bits. windows-sys exposes these as plain u16 constants in
// a module the Windows adapter does not otherwise need, so they are named here
// (and shared with the Windows glue, which re-imports them from this module).
pub(crate) const FOREGROUND_BLUE: u16 = 0x0001;
pub(crate) const FOREGROUND_GREEN: u16 = 0x0002;
pub(crate) const FOREGROUND_RED: u16 = 0x0004;
pub(crate) const FOREGROUND_INTENSITY: u16 = 0x0008;
pub(crate) const BACKGROUND_BLUE: u16 = 0x0010;
pub(crate) const BACKGROUND_GREEN: u16 = 0x0020;
pub(crate) const BACKGROUND_RED: u16 = 0x0040;
pub(crate) const BACKGROUND_INTENSITY: u16 = 0x0080;
// The leading/trailing double-width byte-order bits are consumed only by the
// Windows cell decoder, so they live in `windows::console_agent` rather than
// here; keeping them off this always-compiled module avoids a dead-code warning
// on non-Windows hosts.
pub(crate) const COMMON_LVB_REVERSE_VIDEO: u16 = 0x4000;
pub(crate) const COMMON_LVB_UNDERSCORE: u16 = 0x8000;

pub(crate) const DEFAULT_ATTRIBUTES: u16 = FOREGROUND_RED | FOREGROUND_GREEN | FOREGROUND_BLUE;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Cell {
    pub(crate) text: char,
    pub(crate) attributes: u16,
    /// A wide character's second cell, which carries no glyph of its own.
    /// Set from the console's `COMMON_LVB_TRAILING_BYTE`; unreliable on legacy
    /// consoles, which is why emission also derives continuation from width.
    pub(crate) continuation: bool,
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            text: ' ',
            attributes: DEFAULT_ATTRIBUTES,
            continuation: false,
        }
    }
}

/// Whether a character is East-Asian wide / fullwidth and therefore lays across
/// two console cells.
///
/// This is deliberately the exact predicate the vendored vt100 parser uses when
/// it advances the cursor for a printed character (`c.width().unwrap_or(1) > 1`
/// in `third_party/vt100`). Agreeing with the parser is the whole point: it
/// makes the number of console cells the agent skips equal the number of columns
/// the parser advances, so the two can never drift apart.
pub(crate) fn is_wide(c: char) -> bool {
    UnicodeWidthChar::width(c).unwrap_or(1) > 1
}

/// Appends the text and SGR bytes for one already-positioned, already-erased
/// row, updating the running attribute state.
///
/// The caller is expected to have already written the cursor-home and
/// erase-to-end-of-line prefix (`\x1b[{row};1H\x1b[K`); this function only emits
/// the row's content. `current_attributes` is the attribute state the terminal
/// is currently in and is advanced as SGR sequences are written, so it carries
/// across rows exactly as the previous inline loop did.
///
/// Continuation cells are skipped two ways, on purpose:
/// 1. Any cell already flagged `continuation` (the console set the LVB bit) is
///    skipped — the modern-console path, unchanged.
/// 2. After a glyph whose display width is 2, the *next* cell is skipped as its
///    continuation regardless of that cell's flag or content — the legacy path,
///    where the bit is missing. A wide glyph in the last column has no following
///    cell and is simply emitted once.
pub(crate) fn emit_row_cells(out: &mut Vec<u8>, cells: &[Cell], current_attributes: &mut u16) {
    let mut text = String::new();
    let mut index = 0;
    while index < cells.len() {
        let cell = cells[index];
        if cell.continuation {
            // The console flagged this as a wide glyph's trailing cell; its
            // glyph was already emitted with the leading cell. Skip it.
            index += 1;
            continue;
        }
        if cell.attributes != *current_attributes {
            if !text.is_empty() {
                out.extend_from_slice(text.as_bytes());
                text.clear();
            }
            out.extend_from_slice(sgr_for(cell.attributes).as_bytes());
            *current_attributes = cell.attributes;
        }
        text.push(cell.text);
        // Width-derived continuation: a wide glyph occupies two cells, so the
        // following cell is its continuation whether or not the console said so.
        // Skip it. In the last column there is no following cell to skip.
        if is_wide(cell.text) && index + 1 < cells.len() {
            index += 2;
        } else {
            index += 1;
        }
    }
    // Trailing blanks are already handled by the caller's erase, so they are
    // only written when a later cell on the row is non-blank.
    while text.ends_with(' ') {
        text.pop();
    }
    out.extend_from_slice(text.as_bytes());
}

/// Console attribute bits to an SGR sequence.
///
/// The console orders its colour bits blue-green-red and ANSI orders them
/// red-green-blue, so the two nibbles are not interchangeable and swapping
/// red and blue is the entire mapping.
pub(crate) fn sgr_for(attributes: u16) -> String {
    let ansi = |red: bool, green: bool, blue: bool| {
        u8::from(red) | (u8::from(green) << 1) | (u8::from(blue) << 2)
    };
    let foreground = ansi(
        attributes & FOREGROUND_RED != 0,
        attributes & FOREGROUND_GREEN != 0,
        attributes & FOREGROUND_BLUE != 0,
    );
    let background = ansi(
        attributes & BACKGROUND_RED != 0,
        attributes & BACKGROUND_GREEN != 0,
        attributes & BACKGROUND_BLUE != 0,
    );
    let foreground = if attributes & FOREGROUND_INTENSITY != 0 {
        90 + u16::from(foreground)
    } else {
        30 + u16::from(foreground)
    };
    let background = if attributes & BACKGROUND_INTENSITY != 0 {
        100 + u16::from(background)
    } else {
        40 + u16::from(background)
    };
    let mut sequence = String::from("\x1b[0");
    if attributes & COMMON_LVB_REVERSE_VIDEO != 0 {
        sequence.push_str(";7");
    }
    if attributes & COMMON_LVB_UNDERSCORE != 0 {
        sequence.push_str(";4");
    }
    sequence.push_str(&format!(";{foreground};{background}m"));
    sequence
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A leading + trailing pair for a wide glyph, as the console *should*
    /// report it: both cells carry the same code unit, the second flagged.
    fn wide_pair(c: char) -> [Cell; 2] {
        [
            Cell {
                text: c,
                attributes: DEFAULT_ATTRIBUTES,
                continuation: false,
            },
            Cell {
                text: c,
                attributes: DEFAULT_ATTRIBUTES,
                continuation: true,
            },
        ]
    }

    /// A wide glyph as a *legacy* console reports it: the trailing cell's LVB
    /// bit is unset, and it holds a space (or a duplicate). This is the bug.
    fn wide_pair_no_bit(c: char, trailing: char) -> [Cell; 2] {
        [
            Cell {
                text: c,
                attributes: DEFAULT_ATTRIBUTES,
                continuation: false,
            },
            Cell {
                text: trailing,
                attributes: DEFAULT_ATTRIBUTES,
                continuation: false,
            },
        ]
    }

    fn ascii(c: char) -> Cell {
        Cell {
            text: c,
            attributes: DEFAULT_ATTRIBUTES,
            continuation: false,
        }
    }

    fn emit(cells: &[Cell]) -> String {
        let mut out = Vec::new();
        let mut attrs = DEFAULT_ATTRIBUTES;
        emit_row_cells(&mut out, cells, &mut attrs);
        String::from_utf8(out).expect("utf8")
    }

    #[test]
    fn the_width_oracle_matches_the_vt100_parser_predicate() {
        // Exactly `c.width().unwrap_or(1) > 1`, the vendored vt100 rule.
        assert!(is_wide('中'));
        assert!(is_wide('文'));
        assert!(is_wide('Ａ'), "fullwidth Latin A is wide");
        assert!(!is_wide('a'));
        assert!(!is_wide(' '));
        assert!(!is_wide('é'));
    }

    #[test]
    fn a_pure_cjk_row_emits_each_wide_char_once() {
        let mut cells = Vec::new();
        cells.extend_from_slice(&wide_pair('中'));
        cells.extend_from_slice(&wide_pair('文'));
        cells.extend_from_slice(&wide_pair('汉'));
        cells.extend_from_slice(&wide_pair('字'));
        let rendered = emit(&cells);
        assert!(rendered.ends_with("中文汉字"), "{rendered:?}");
    }

    #[test]
    fn mixed_cjk_and_ascii_does_not_drift() {
        // 中 a 文 b — the exact shape the user reported drifting.
        let mut cells = Vec::new();
        cells.extend_from_slice(&wide_pair('中'));
        cells.push(ascii('a'));
        cells.extend_from_slice(&wide_pair('文'));
        cells.push(ascii('b'));
        let rendered = emit(&cells);
        assert!(rendered.ends_with("中a文b"), "{rendered:?}");
    }

    /// The actual bug: legacy console, continuation bit NOT set, trailing cell
    /// a space. Width-derived continuation must still skip it.
    #[test]
    fn a_wide_char_is_skipped_even_when_the_console_forgot_the_bit_space() {
        let mut cells = Vec::new();
        cells.extend_from_slice(&wide_pair_no_bit('中', ' '));
        cells.extend_from_slice(&wide_pair_no_bit('文', ' '));
        let rendered = emit(&cells);
        assert!(
            rendered.ends_with("中文"),
            "no stray space between the characters: {rendered:?}"
        );
    }

    /// Same bug, but the trailing cell duplicates the code unit rather than
    /// holding a space — the other way legacy consoles fill it.
    #[test]
    fn a_wide_char_is_skipped_even_when_the_console_forgot_the_bit_duplicate() {
        let mut cells = Vec::new();
        cells.extend_from_slice(&wide_pair_no_bit('中', '中'));
        cells.push(ascii('X'));
        let rendered = emit(&cells);
        assert!(rendered.ends_with("中X"), "no doubled glyph: {rendered:?}");
    }

    /// Mixed row where the trailing cells are unflagged: without the width
    /// oracle every wide char would gain a stray cell and the row would drift.
    #[test]
    fn mixed_row_with_missing_bits_still_aligns() {
        let mut cells = Vec::new();
        cells.extend_from_slice(&wide_pair_no_bit('汉', ' '));
        cells.push(ascii('1'));
        cells.push(ascii('2'));
        cells.extend_from_slice(&wide_pair_no_bit('字', ' '));
        cells.push(ascii('3'));
        let rendered = emit(&cells);
        assert!(rendered.ends_with("汉12字3"), "{rendered:?}");
    }

    /// Modern console that *does* set the bit must still work and must not skip
    /// twice (which would eat the following real character).
    #[test]
    fn a_flagged_continuation_is_not_double_skipped() {
        let mut cells = Vec::new();
        cells.extend_from_slice(&wide_pair('中'));
        cells.push(ascii('a'));
        let rendered = emit(&cells);
        assert!(rendered.ends_with("中a"), "{rendered:?}");
    }

    /// A wide glyph in the final column has no trailing cell to skip.
    #[test]
    fn a_wide_char_in_the_last_column_emits_once() {
        let cells = vec![ascii('a'), ascii('b'), ascii('c'), ascii('中')];
        let rendered = emit(&cells);
        assert!(rendered.ends_with("abc中"), "{rendered:?}");
    }

    /// A leading wide cell present but the row ends right after it (odd width).
    #[test]
    fn a_lone_leading_wide_cell_at_row_end_emits_once() {
        let cells = vec![ascii('x'), ascii('中')];
        let rendered = emit(&cells);
        assert!(rendered.ends_with("x中"), "{rendered:?}");
    }

    /// Ambiguous-width characters are treated as narrow (width 1), matching the
    /// vt100 parser, so they are emitted once and consume one cell.
    #[test]
    fn an_ambiguous_width_char_is_treated_as_narrow() {
        // U+00A7 SECTION SIGN is East-Asian Ambiguous; unicode-width reports 1.
        assert!(!is_wide('\u{00A7}'));
        let cells = vec![ascii('\u{00A7}'), ascii('z')];
        let rendered = emit(&cells);
        assert!(rendered.ends_with("\u{00A7}z"), "{rendered:?}");
    }

    #[test]
    fn a_blank_row_emits_nothing_after_the_caller_prefix() {
        let cells = vec![Cell::default(); 5];
        let rendered = emit(&cells);
        assert_eq!(rendered, "", "trailing blanks ride on the erase");
    }

    #[test]
    fn trailing_blanks_are_trimmed_but_interior_blanks_are_kept() {
        let cells = vec![ascii('h'), ascii('i'), ascii(' '), ascii('!'), ascii(' ')];
        let rendered = emit(&cells);
        assert!(rendered.ends_with("hi !"), "{rendered:?}");
        assert!(!rendered.ends_with("hi !  "), "{rendered:?}");
    }

    /// An attribute change flushes accumulated text and writes an SGR sequence
    /// before the styled run — the behaviour the inline loop had.
    #[test]
    fn an_attribute_change_flushes_text_and_writes_sgr() {
        let mut cells = vec![ascii('a')];
        cells.push(Cell {
            text: 'b',
            attributes: FOREGROUND_RED,
            continuation: false,
        });
        let mut out = Vec::new();
        let mut attrs = DEFAULT_ATTRIBUTES;
        emit_row_cells(&mut out, &cells, &mut attrs);
        let rendered = String::from_utf8(out).expect("utf8");
        assert!(rendered.starts_with('a'), "{rendered:?}");
        assert!(
            rendered.contains("\x1b[0"),
            "an SGR sequence appears: {rendered:?}"
        );
        assert!(rendered.ends_with('b'), "{rendered:?}");
        assert_eq!(attrs, FOREGROUND_RED, "running attribute state advanced");
    }

    /// The trailing cell of a wide glyph is never inspected for attributes, so a
    /// stray attribute on the (skipped) continuation cell emits no SGR.
    #[test]
    fn a_skipped_continuation_cell_does_not_emit_its_attributes() {
        let cells = [
            Cell {
                text: '中',
                attributes: DEFAULT_ATTRIBUTES,
                continuation: false,
            },
            Cell {
                text: ' ',
                attributes: FOREGROUND_RED, // would emit SGR if not skipped
                continuation: false,
            },
        ];
        let rendered = emit(&cells);
        assert_eq!(rendered, "中", "no SGR from the skipped cell: {rendered:?}");
    }
}
