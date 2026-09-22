//! Host-neutral interaction and rendering primitives shared by AgenTerm UIs.

pub mod damage;
pub mod glyph_cache;
pub mod pixel;
pub mod retained_frame;
#[cfg(feature = "terminal-selection")]
pub mod terminal_selection;
pub mod tree;

pub use damage::{DirtyRegion, DirtyRows, PixelRect};
pub use glyph_cache::{GlyphCache, GlyphCacheKey, GlyphCacheStats};
pub use retained_frame::{RetainedFrameError, RetainedXrgbFrame};
pub use tree::{TreeDepthError, TreeDepthNode, compute_tree_depths, compute_tree_depths_by};

const MIN_THUMB_HEIGHT: i32 = 24;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Rect {
    pub const fn width(self) -> i32 {
        self.right - self.left
    }
    pub const fn height(self) -> i32 {
        self.bottom - self.top
    }
    pub const fn contains(self, x: i32, y: i32) -> bool {
        x >= self.left && x < self.right && y >= self.top && y < self.bottom
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScrollbarGeometry {
    pub track: Rect,
    pub thumb: Rect,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScrollbarHit {
    Thumb,
    TrackAbove,
    TrackBelow,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ScrollbarThumbDrag {
    grab: i32,
}

impl ScrollbarThumbDrag {
    pub const fn begin(pointer_y: i32, thumb_top: i32) -> Self {
        Self {
            grab: pointer_y - thumb_top,
        }
    }
    pub const fn thumb_top(self, pointer_y: i32) -> i32 {
        pointer_y - self.grab
    }
}

pub fn terminal_scrollbar_geometry(
    terminal: Rect,
    width: i32,
    visible: usize,
    offset: usize,
    maximum: usize,
) -> ScrollbarGeometry {
    let track = Rect {
        left: (terminal.right - width.max(0)).max(terminal.left),
        ..terminal
    };
    let height = track.height().max(0);
    let total = visible.saturating_add(maximum).max(1);
    let proportional = (i64::from(height) * visible.max(1) as i64 / total as i64) as i32;
    let thumb_height = if maximum == 0 {
        height
    } else {
        proportional.max(MIN_THUMB_HEIGHT).min(height)
    };
    let travel = (height - thumb_height).max(0);
    let from_bottom = if maximum == 0 {
        0
    } else {
        (offset.min(maximum) as i64 * i64::from(travel) / maximum as i64) as i32
    };
    let top = track.bottom - thumb_height - from_bottom;
    ScrollbarGeometry {
        track,
        thumb: Rect {
            left: (track.left + 2).min(track.right),
            top,
            right: (track.right - 2).max((track.left + 2).min(track.right)),
            bottom: top + thumb_height,
        },
    }
}

pub fn scrollback_for_thumb_top(g: ScrollbarGeometry, top: i32, maximum: usize) -> usize {
    let travel = g.track.height() - g.thumb.height();
    if maximum == 0 || travel <= 0 {
        return 0;
    }
    let top = top.clamp(g.track.top, g.track.bottom - g.thumb.height());
    let from_bottom = g.track.bottom - g.thumb.height() - top;
    ((i64::from(from_bottom) * maximum as i64 + i64::from(travel) / 2) / i64::from(travel)) as usize
}

pub fn scrollbar_hit_test(g: &ScrollbarGeometry, x: i32, y: i32) -> Option<ScrollbarHit> {
    if !g.track.contains(x, y) {
        None
    } else if g.thumb.contains(x, y) {
        Some(ScrollbarHit::Thumb)
    } else if y < g.thumb.top {
        Some(ScrollbarHit::TrackAbove)
    } else {
        Some(ScrollbarHit::TrackBelow)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn rect() -> Rect {
        Rect {
            left: 200,
            top: 0,
            right: 1000,
            bottom: 600,
        }
    }

    #[test]
    fn bottom_middle_top_round_trip() {
        let bottom = terminal_scrollbar_geometry(rect(), 12, 30, 0, 90);
        let middle = terminal_scrollbar_geometry(rect(), 12, 30, 45, 90);
        let top = terminal_scrollbar_geometry(rect(), 12, 30, 90, 90);
        assert_eq!(bottom.track.left, 988);
        assert!(bottom.thumb.top > middle.thumb.top && middle.thumb.top > top.thumb.top);
        assert_eq!(scrollback_for_thumb_top(middle, middle.thumb.top, 90), 45);
    }

    #[test]
    fn drag_keeps_grab_offset() {
        let g = terminal_scrollbar_geometry(rect(), 12, 30, 45, 90);
        assert_eq!(
            scrollbar_hit_test(&g, g.thumb.left, g.thumb.top),
            Some(ScrollbarHit::Thumb)
        );
        let drag = ScrollbarThumbDrag::begin(g.thumb.top + 5, g.thumb.top);
        assert_eq!(drag.thumb_top(g.thumb.top + 25), g.thumb.top + 20);
    }

    // Parity vectors shared with MiniCon, which carried the same arithmetic
    // in its own crate. Copied verbatim (ScrollbarRect read as Rect) so both
    // products are held to one behaviour before MiniCon drops its copy.

    #[test]
    fn thumb_positions_span_the_track_from_bottom_to_top() {
        let bottom = terminal_scrollbar_geometry(rect(), 12, 30, 0, 90);
        let middle = terminal_scrollbar_geometry(rect(), 12, 30, 45, 90);
        let top = terminal_scrollbar_geometry(rect(), 12, 30, 90, 90);
        assert_eq!(bottom.thumb.bottom, bottom.track.bottom);
        assert_eq!(top.thumb.top, top.track.top);
        assert!(middle.thumb.top < bottom.thumb.top);
        assert!(middle.thumb.top > top.thumb.top);
        // The thumb's vertical extent never changes while scrolling.
        assert_eq!(bottom.thumb.height(), middle.thumb.height());
        assert_eq!(middle.thumb.height(), top.thumb.height());
    }

    #[test]
    fn thumb_position_round_trips_through_the_scroll_offset() {
        for maximum in [1_usize, 7, 90, 1000] {
            let geometry = terminal_scrollbar_geometry(rect(), 12, 30, 0, maximum);
            for offset in [0, maximum / 3, maximum / 2, maximum] {
                let drawn = terminal_scrollbar_geometry(rect(), 12, 30, offset, maximum);
                let back = scrollback_for_thumb_top(geometry, drawn.thumb.top, maximum);
                // Rounding may move a value by at most a row's worth of travel;
                // anything larger means the two directions disagree.
                let tolerance = (maximum / 30).max(1);
                assert!(
                    back.abs_diff(offset) <= tolerance,
                    "offset {offset} of {maximum} came back as {back}"
                );
            }
        }
    }

    #[test]
    fn every_offset_returns_exactly_when_each_row_has_its_own_pixel() {
        // The loose round trip above tolerates a row of drift; this one does
        // not. The forward map truncates, so the inverse must round, and that
        // is exact only with more than two pixels of travel per row. Below
        // that ratio a press on the thumb can land one row away.
        for maximum in [7_usize, 90, 200] {
            let geometry = terminal_scrollbar_geometry(rect(), 12, 30, 0, maximum);
            assert!(geometry.track.height() - geometry.thumb.height() > 2 * maximum as i32);
            for offset in 0..=maximum {
                let drawn = terminal_scrollbar_geometry(rect(), 12, 30, offset, maximum);
                assert_eq!(
                    scrollback_for_thumb_top(geometry, drawn.thumb.top, maximum),
                    offset,
                    "offset {offset} of {maximum}"
                );
            }
        }
    }

    #[test]
    fn a_huge_scrollback_keeps_the_thumb_grabbable() {
        let geometry = terminal_scrollbar_geometry(rect(), 12, 30, 0, 1_000_000);
        // A literal, not the constant: the floor is the behaviour under test.
        assert_eq!(geometry.thumb.height(), 24);
    }

    #[test]
    fn a_full_track_thumb_never_scrolls() {
        let geometry = terminal_scrollbar_geometry(rect(), 12, 30, 0, 0);
        assert_eq!(geometry.thumb.top, geometry.track.top);
        assert_eq!(geometry.thumb.height(), geometry.track.height());
        assert_eq!(scrollback_for_thumb_top(geometry, 0, 0), 0);
    }

    /// A track shorter than the initial carve-out would otherwise produce a
    /// rectangle whose right edge is left of its left edge, and a negative
    /// height would make every downstream comparison nonsense.
    #[test]
    fn a_track_narrower_than_the_inset_stays_ordered() {
        for width in 0..8 {
            for track_width in 0..8 {
                let narrow = Rect {
                    left: 0,
                    top: 0,
                    right: track_width,
                    bottom: 40,
                };
                let geometry = terminal_scrollbar_geometry(narrow, width, 10, 0, 10);
                assert!(
                    geometry.thumb.right >= geometry.thumb.left,
                    "width {width} track {track_width}"
                );
                assert!(geometry.thumb.bottom >= geometry.thumb.top);
            }
        }
    }

    /// The track is passed through as given, so an inverted rectangle stays
    /// inverted: `height()` is negative rather than clamped to zero. Callers
    /// therefore cannot treat a non-negative `height()` as an invariant they
    /// get for free, and the drawing path has to guard its own arithmetic.
    /// Recorded here because it is surprising and easy to assume otherwise.
    // Behaviour that was real but undocumented until MiniCon pinned it:
    // callers must not assume a non-negative track height.
    #[test]
    fn an_inverted_track_is_passed_through_with_a_negative_height() {
        let inverted = Rect {
            left: 100,
            top: 60,
            right: 40,
            bottom: 0,
        };
        let geometry = terminal_scrollbar_geometry(inverted, 12, 10, 0, 10);
        assert_eq!(geometry.track, inverted);
        assert_eq!(geometry.track.height(), -60);
        // The clamped locals still keep the thumb degenerate rather than
        // producing a rectangle that wraps.
        assert_eq!(geometry.thumb.height(), 0);
        assert_eq!(scrollback_for_thumb_top(geometry, 0, 10), 0);
    }

    /// A zero-height track has no area for a thumb, and neither direction may
    /// divide by its zero travel.
    // Behaviour that was real but undocumented until MiniCon pinned it.
    #[test]
    fn an_empty_track_yields_no_usable_thumb() {
        let empty = Rect {
            left: 0,
            top: 100,
            right: 20,
            bottom: 100,
        };
        let geometry = terminal_scrollbar_geometry(empty, 12, 10, 0, 10);
        assert_eq!(geometry.track.height(), 0);
        assert_eq!(geometry.thumb.height(), 0);
        assert_eq!(scrollback_for_thumb_top(geometry, 0, 10), 0);
    }

    #[test]
    fn an_oversized_offset_is_clamped_to_the_maximum() {
        let at_max = terminal_scrollbar_geometry(rect(), 12, 30, 90, 90);
        let over = terminal_scrollbar_geometry(rect(), 12, 30, usize::MAX, 90);
        assert_eq!(at_max.thumb.top, over.thumb.top);
    }

    /// The thumb is tested before the track halves so a click on the thumb
    /// starts a drag. With the view at the live end the thumb covers the bottom
    /// of the track, so there is deliberately no `TrackBelow` to hit — a click
    /// below the thumb can only exist once the view is scrolled back.
    #[test]
    fn hit_testing_prefers_the_thumb_and_names_the_track_half() {
        // Scrolled back halfway, so the thumb has track on both sides.
        let geometry = terminal_scrollbar_geometry(rect(), 12, 30, 45, 90);
        let x = geometry.thumb.left;
        let y = geometry.thumb.top + geometry.thumb.height() / 2;
        assert_eq!(
            scrollbar_hit_test(&geometry, x, y),
            Some(ScrollbarHit::Thumb)
        );
        assert_eq!(
            scrollbar_hit_test(&geometry, x, geometry.thumb.top - 1),
            Some(ScrollbarHit::TrackAbove)
        );
        assert_eq!(
            scrollbar_hit_test(&geometry, x, geometry.thumb.bottom),
            Some(ScrollbarHit::TrackBelow)
        );
        // A point outside the track is not a hit at all, so a click in the
        // terminal body is not swallowed by the scrollbar.
        assert_eq!(
            scrollbar_hit_test(&geometry, geometry.track.left - 1, y),
            None
        );
        assert_eq!(
            scrollbar_hit_test(&geometry, x, geometry.track.bottom),
            None
        );
    }

    /// At the live end the thumb rests on the track's bottom edge, so the whole
    /// track above it is `TrackAbove` and nothing is below.
    #[test]
    fn a_live_end_thumb_has_no_track_below_it() {
        let geometry = terminal_scrollbar_geometry(rect(), 12, 30, 0, 90);
        let x = geometry.thumb.left;
        assert_eq!(geometry.thumb.bottom, geometry.track.bottom);
        assert_eq!(
            scrollbar_hit_test(&geometry, x, geometry.track.top),
            Some(ScrollbarHit::TrackAbove)
        );
        assert_eq!(
            scrollbar_hit_test(&geometry, x, geometry.thumb.bottom),
            None,
            "the bottom edge of the track is outside it"
        );
    }

    #[test]
    fn a_drag_keeps_the_grab_offset_so_the_thumb_does_not_jump() {
        let drag = ScrollbarThumbDrag::begin(500, 480);
        assert_eq!(drag.thumb_top(500), 480);
        assert_eq!(drag.thumb_top(520), 500);
        assert_eq!(drag.thumb_top(480), 460);
    }
}
