//! Pure terminal-cell selection semantics shared by terminal products.

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct TerminalPoint {
    pub row: u16,
    pub col: u16,
}

pub const fn normalize_endpoints(
    first: TerminalPoint,
    second: TerminalPoint,
) -> (TerminalPoint, TerminalPoint) {
    if first.row < second.row || (first.row == second.row && first.col <= second.col) {
        (first, second)
    } else {
        (second, first)
    }
}

pub const fn clamp_point(point: TerminalPoint, rows: u16, cols: u16) -> Option<TerminalPoint> {
    if rows == 0 || cols == 0 {
        return None;
    }
    Some(TerminalPoint {
        row: if point.row < rows {
            point.row
        } else {
            rows - 1
        },
        col: if point.col < cols {
            point.col
        } else {
            cols - 1
        },
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CellWordClass {
    Whitespace,
    Word,
    Punctuation(char),
}

pub fn word_selection(
    screen: &vt100::Screen,
    point: TerminalPoint,
) -> Option<(TerminalPoint, TerminalPoint)> {
    let (rows, cols) = screen.size();
    let point = clamp_point(point, rows, cols)?;
    let mut clicked_col = point.col;
    while clicked_col > 0
        && screen
            .cell(point.row, clicked_col)
            .is_some_and(vt100::Cell::is_wide_continuation)
    {
        clicked_col -= 1;
    }

    let clicked_class = cell_word_class(screen, point.row, clicked_col);
    let mut start = clicked_col;
    while start > 0 {
        let previous = previous_cell_start(screen, point.row, start);
        if cell_word_class(screen, point.row, previous) != clicked_class {
            break;
        }
        start = previous;
    }

    let mut end_start = clicked_col;
    while let Some(next) = next_cell_start(screen, point.row, end_start, cols) {
        if cell_word_class(screen, point.row, next) != clicked_class {
            break;
        }
        end_start = next;
    }
    let end = cell_end(screen, point.row, end_start, cols);
    Some((
        TerminalPoint {
            row: point.row,
            col: start,
        },
        TerminalPoint {
            row: point.row,
            col: end,
        },
    ))
}

pub fn visible_row_selection(
    screen: &vt100::Screen,
    row: u16,
) -> Option<(TerminalPoint, TerminalPoint)> {
    let (rows, cols) = screen.size();
    let point = clamp_point(TerminalPoint { row, col: 0 }, rows, cols)?;
    Some((
        point,
        TerminalPoint {
            row: point.row,
            col: cols - 1,
        },
    ))
}

pub fn terminal_selection_text(
    screen: &vt100::Screen,
    first: TerminalPoint,
    second: TerminalPoint,
) -> String {
    let (rows, cols) = screen.size();
    let (Some(first), Some(second)) = (
        clamp_point(first, rows, cols),
        clamp_point(second, rows, cols),
    ) else {
        return String::new();
    };
    let (start, end) = normalize_endpoints(first, second);
    let mut selected = String::new();
    for row in start.row..=end.row {
        let first_col = if row == start.row { start.col } else { 0 };
        let last_col = if row == end.row { end.col } else { cols - 1 };
        let mut line = String::new();
        for col in first_col..=last_col {
            match screen.cell(row, col) {
                Some(cell) if cell.is_wide_continuation() => {}
                Some(cell) if cell.has_contents() => line.push_str(cell.contents()),
                _ => line.push(' '),
            }
        }
        selected.push_str(line.trim_end_matches(' '));
        if row != end.row {
            selected.push_str("\r\n");
        }
    }
    selected
}

fn cell_word_class(screen: &vt100::Screen, row: u16, col: u16) -> CellWordClass {
    let contents = screen
        .cell(row, col)
        .map(vt100::Cell::contents)
        .unwrap_or_default();
    let Some(first) = contents.chars().next() else {
        return CellWordClass::Whitespace;
    };
    if contents.chars().all(char::is_whitespace) {
        CellWordClass::Whitespace
    } else if contents.chars().any(is_terminal_word_character) {
        CellWordClass::Word
    } else {
        CellWordClass::Punctuation(first)
    }
}

fn is_terminal_word_character(character: char) -> bool {
    character.is_alphanumeric()
        || matches!(character, '_' | '-' | '.' | '/' | '\\' | ':' | '@' | '~')
}

fn previous_cell_start(screen: &vt100::Screen, row: u16, col: u16) -> u16 {
    debug_assert!(col > 0, "previous_cell_start requires col > 0");
    let mut previous = col - 1;
    while previous > 0
        && screen
            .cell(row, previous)
            .is_some_and(vt100::Cell::is_wide_continuation)
    {
        previous -= 1;
    }
    previous
}

fn next_cell_start(screen: &vt100::Screen, row: u16, col: u16, cols: u16) -> Option<u16> {
    let next = cell_end(screen, row, col, cols).saturating_add(1);
    (next < cols).then_some(next)
}

fn cell_end(screen: &vt100::Screen, row: u16, col: u16, cols: u16) -> u16 {
    if col + 1 < cols
        && screen
            .cell(row, col + 1)
            .is_some_and(vt100::Cell::is_wide_continuation)
    {
        col + 1
    } else {
        col
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_follow_terminal_cells_and_wide_spans() {
        let mut parser = vt100::Parser::new(2, 24, 0);
        parser.process("alpha.rs 你好 ok!".as_bytes());
        assert_eq!(
            word_selection(parser.screen(), TerminalPoint { row: 0, col: 10 }),
            Some((
                TerminalPoint { row: 0, col: 9 },
                TerminalPoint { row: 0, col: 12 },
            ))
        );
        assert_eq!(
            word_selection(parser.screen(), TerminalPoint { row: 0, col: 16 }),
            Some((
                TerminalPoint { row: 0, col: 16 },
                TerminalPoint { row: 0, col: 16 },
            ))
        );
    }

    #[test]
    fn text_normalizes_direction_crlf_and_wide_continuations() {
        let mut parser = vt100::Parser::new(3, 8, 0);
        parser.process("你A\r\nbeta".as_bytes());
        let start = TerminalPoint { row: 0, col: 0 };
        let end = TerminalPoint { row: 1, col: 3 };
        assert_eq!(
            terminal_selection_text(parser.screen(), start, end),
            "你A\r\nbeta"
        );
        assert_eq!(
            terminal_selection_text(parser.screen(), end, start),
            "你A\r\nbeta"
        );
    }

    #[test]
    fn visible_row_is_clamped_without_following_soft_wraps() {
        let mut parser = vt100::Parser::new(3, 8, 0);
        parser.process(b"abcdefghijkl");
        assert!(parser.screen().row_wrapped(0));
        assert_eq!(
            visible_row_selection(parser.screen(), 1),
            Some((
                TerminalPoint { row: 1, col: 0 },
                TerminalPoint { row: 1, col: 7 },
            ))
        );
    }

    /// `Screen::rows` walks the scrollback-aware row iterator while
    /// `Screen::cell` indexes one `visible_row` directly. After `visible_row`
    /// stopped rebuilding that iterator per lookup (it was the hot path in
    /// every cell painter), the two must still agree at every scrollback
    /// offset — including offsets larger than the screen, where the visible
    /// rows come entirely from history.
    #[test]
    fn indexed_cells_agree_with_the_row_iteration_at_every_offset() {
        let (rows, cols) = (4_u16, 12_u16);
        let mut parser = vt100::Parser::new(rows, cols, 100);
        for line in 0..30u16 {
            // Short enough to never soft-wrap, so one logical line is one row.
            parser.process(format!("r{line:02}\r\n").as_bytes());
        }
        for offset in [0_usize, 1, 2, 3, 4, 15, 29, 45, 120] {
            parser.screen_mut().set_scrollback(offset);
            let iterated: Vec<String> = parser.screen().rows(0, cols).collect();
            for row in 0..rows {
                let indexed: String = (0..cols)
                    .filter_map(|col| parser.screen().cell(row, col))
                    .filter(|cell| !cell.is_wide_continuation())
                    .map(vt100::Cell::contents)
                    .collect();
                let expected = iterated
                    .get(usize::from(row))
                    .map_or("", std::string::String::as_str);
                assert_eq!(
                    indexed.trim_end(),
                    expected.trim_end(),
                    "offset {offset} row {row}: indexed cells diverged from rows()"
                );
            }
        }
    }
}
