//! Platform-neutral window enumeration contract.

use std::borrow::Cow;

/// Bounds of a top-level window in physical screen pixels (top-origin).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct WindowBounds {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// One display in top-origin coordinates (same space as [`WindowBounds`]).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ScreenInfo {
    pub frame: WindowBounds,
    pub visible: WindowBounds,
    pub primary: bool,
}

/// A snapshot of one visible top-level window.
#[derive(Clone, Debug, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct WindowInfo {
    /// Native window handle (HWND on Windows), valid for the observation instant.
    pub handle: isize,
    pub title: String,
    pub process_id: u32,
    pub app_name: String,
    pub bounds: WindowBounds,
    pub focused: bool,
    pub minimized: bool,
}

/// Where one window sits in the desktop's front-to-back order, and how much
/// of it the windows in front actually cover.
///
/// Both numbers describe **one observation instant** and mean nothing
/// across two of them: a window's `z_index` changes the moment anything is
/// raised. A backend that cannot report a real stacking order must answer
/// `Unsupported` rather than pass its enumeration order off as one --
/// creation order looks exactly like stacking order until it silently
/// isn't.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct WindowStacking {
    pub handle: isize,
    /// 0 is frontmost.
    pub z_index: u32,
    /// How much of this window's area the windows in front of it cover, in
    /// percent (0..=100). 100 means nothing of it is visible. Computed from
    /// the rectangles, not sampled from the screen, so it is exact for
    /// rectangular windows and an over-estimate for rounded corners.
    pub occluded_percent: u32,
}

/// Front-to-back stacking for a list of windows already in that order.
///
/// The covered area is computed by subtracting each front window's
/// rectangle from what is still uncovered, which is exact for axis-aligned
/// rectangles -- no sampling, no grid, no screenshot. An empty or degenerate
/// window reports 100: there is nothing of it to see.
pub fn stacking_from_front_to_back(windows: &[(isize, WindowBounds)]) -> Vec<WindowStacking> {
    let mut out = Vec::with_capacity(windows.len());
    for (index, (handle, bounds)) in windows.iter().enumerate() {
        let area = i64::from(bounds.width) * i64::from(bounds.height);
        let occluded_percent = if area <= 0 {
            100
        } else {
            let mut uncovered = vec![*bounds];
            for (_, front) in &windows[..index] {
                let mut next = Vec::new();
                for piece in uncovered {
                    subtract(piece, *front, &mut next);
                }
                uncovered = next;
                if uncovered.is_empty() {
                    break;
                }
            }
            let visible: i64 = uncovered
                .iter()
                .map(|rect| i64::from(rect.width) * i64::from(rect.height))
                .sum();
            let covered = (area - visible).max(0);
            // Round to nearest so "almost entirely covered" does not read
            // as 99 forever, and a single uncovered pixel never reads 100.
            let percent = ((covered * 200 + area) / (area * 2)).clamp(0, 100) as u32;
            if percent == 100 && visible > 0 {
                99
            } else {
                percent
            }
        };
        out.push(WindowStacking {
            handle: *handle,
            z_index: index as u32,
            occluded_percent,
        });
    }
    out
}

/// Push the parts of `piece` that `cover` does not overlap. Up to four
/// pieces (above, below, left, right of the intersection).
fn subtract(piece: WindowBounds, cover: WindowBounds, out: &mut Vec<WindowBounds>) {
    let (px0, py0) = (piece.x, piece.y);
    let px1 = piece.x.saturating_add_unsigned(piece.width);
    let py1 = piece.y.saturating_add_unsigned(piece.height);
    let (cx0, cy0) = (cover.x, cover.y);
    let cx1 = cover.x.saturating_add_unsigned(cover.width);
    let cy1 = cover.y.saturating_add_unsigned(cover.height);
    let ix0 = px0.max(cx0);
    let iy0 = py0.max(cy0);
    let ix1 = px1.min(cx1);
    let iy1 = py1.min(cy1);
    if ix0 >= ix1 || iy0 >= iy1 {
        out.push(piece);
        return;
    }
    let band = |x0: i32, y0: i32, x1: i32, y1: i32, out: &mut Vec<WindowBounds>| {
        if x1 > x0 && y1 > y0 {
            out.push(WindowBounds {
                x: x0,
                y: y0,
                width: (x1 - x0) as u32,
                height: (y1 - y0) as u32,
            });
        }
    };
    band(px0, py0, px1, iy0, out);
    band(px0, iy1, px1, py1, out);
    band(px0, iy0, ix0, iy1, out);
    band(ix1, iy0, px1, iy1, out);
}

#[cfg(test)]
mod stacking_tests {
    use super::*;

    fn rect(x: i32, y: i32, width: u32, height: u32) -> WindowBounds {
        WindowBounds {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn the_frontmost_window_is_never_occluded_and_indices_run_front_to_back() {
        let rows =
            stacking_from_front_to_back(&[(1, rect(0, 0, 100, 100)), (2, rect(0, 0, 100, 100))]);
        assert_eq!(rows[0].z_index, 0);
        assert_eq!(rows[0].occluded_percent, 0);
        assert_eq!(rows[1].z_index, 1);
        // Exactly covered by the window in front of it.
        assert_eq!(rows[1].occluded_percent, 100);
    }

    #[test]
    fn partial_overlap_is_the_covered_area_not_a_yes_or_no() {
        // A 100x100 window with a 50x100 strip covered: half of it.
        let rows =
            stacking_from_front_to_back(&[(1, rect(50, 0, 50, 100)), (2, rect(0, 0, 100, 100))]);
        assert_eq!(rows[1].occluded_percent, 50);
    }

    #[test]
    fn two_overlapping_covers_are_not_counted_twice() {
        // Two front windows each covering half, overlapping in the middle
        // 20 columns: 0..60 and 40..100 cover the whole 100 wide window.
        let rows = stacking_from_front_to_back(&[
            (1, rect(0, 0, 60, 100)),
            (2, rect(40, 0, 60, 100)),
            (3, rect(0, 0, 100, 100)),
        ]);
        assert_eq!(rows[2].occluded_percent, 100);
        // The second window is itself covered only by the first, on the
        // 40..60 strip: 20 of its 60 columns.
        assert_eq!(rows[1].occluded_percent, 33);
    }

    #[test]
    fn a_window_beside_another_is_not_occluded_by_it() {
        let rows =
            stacking_from_front_to_back(&[(1, rect(0, 0, 100, 100)), (2, rect(200, 0, 100, 100))]);
        assert_eq!(rows[1].occluded_percent, 0);
    }

    #[test]
    fn a_single_visible_pixel_never_rounds_up_to_fully_hidden() {
        // 1 uncovered column of 1000 rounds to 100 without the guard, which
        // would read as "you cannot see any of this window" while a sliver
        // of it is on screen.
        let rows =
            stacking_from_front_to_back(&[(1, rect(0, 0, 999, 100)), (2, rect(0, 0, 1000, 100))]);
        assert_eq!(rows[1].occluded_percent, 99);
    }

    #[test]
    fn an_empty_window_reports_fully_occluded_rather_than_dividing_by_zero() {
        let rows = stacking_from_front_to_back(&[(1, rect(10, 10, 0, 0))]);
        assert_eq!(rows[0].occluded_percent, 100);
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum WindowEnumerateError {
    Unsupported {
        reason: Cow<'static, str>,
    },
    Failed {
        code: Cow<'static, str>,
        message: String,
    },
}

impl WindowEnumerateError {
    pub(crate) fn failed(code: &'static str, message: impl ToString) -> Self {
        Self::Failed {
            code: code.into(),
            message: message.to_string(),
        }
    }
}
