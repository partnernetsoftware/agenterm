//! Framebuffer composition truth: channel order, clipping, and overlap.

use agenterm_vnc::{BYTES_PER_PIXEL, Framebuffer, Rect};

/// Read one pixel as (r, g, b, a).
fn pixel(fb: &Framebuffer, x: u16, y: u16) -> (u8, u8, u8, u8) {
    let i = (y as usize * fb.width() as usize + x as usize) * BYTES_PER_PIXEL;
    let p = &fb.as_rgba()[i..i + BYTES_PER_PIXEL];
    (p[0], p[1], p[2], p[3])
}

/// A tightly packed BGRA rect of one repeated colour.
fn bgra_fill(rect: Rect, b: u8, g: u8, r: u8) -> Vec<u8> {
    let mut data = Vec::with_capacity(rect.byte_len());
    for _ in 0..rect.width as usize * rect.height as usize {
        data.extend_from_slice(&[b, g, r, 0xff]);
    }
    data
}

#[test]
fn new_surface_is_opaque_black() {
    let fb = Framebuffer::new(3, 2);
    assert_eq!(fb.as_rgba().len(), 3 * 2 * BYTES_PER_PIXEL);
    for y in 0..2 {
        for x in 0..3 {
            assert_eq!(pixel(&fb, x, y), (0, 0, 0, 0xff), "pixel {x},{y}");
        }
    }
}

#[test]
fn blit_swaps_bgra_to_rgba() {
    let mut fb = Framebuffer::new(2, 1);
    let rect = Rect { x: 0, y: 0, width: 2, height: 1 };
    // Wire bytes are [b=0x10, g=0x20, r=0x30]; the canvas must see r first.
    fb.blit_bgra(rect, &bgra_fill(rect, 0x10, 0x20, 0x30));
    assert_eq!(pixel(&fb, 0, 0), (0x30, 0x20, 0x10, 0xff));
    assert_eq!(pixel(&fb, 1, 0), (0x30, 0x20, 0x10, 0xff));
}

#[test]
fn blit_lands_at_the_rect_origin() {
    let mut fb = Framebuffer::new(4, 4);
    let rect = Rect { x: 1, y: 2, width: 2, height: 1 };
    fb.blit_bgra(rect, &bgra_fill(rect, 0, 0, 0xff));

    assert_eq!(pixel(&fb, 1, 2), (0xff, 0, 0, 0xff));
    assert_eq!(pixel(&fb, 2, 2), (0xff, 0, 0, 0xff));
    // Neighbours stay untouched — no row wrap, no vertical drift.
    assert_eq!(pixel(&fb, 0, 2), (0, 0, 0, 0xff));
    assert_eq!(pixel(&fb, 3, 2), (0, 0, 0, 0xff));
    assert_eq!(pixel(&fb, 1, 1), (0, 0, 0, 0xff));
}

#[test]
fn blit_clips_instead_of_wrapping_or_panicking() {
    let mut fb = Framebuffer::new(4, 4);
    // A rect that overhangs both the right and bottom edges.
    let rect = Rect { x: 3, y: 3, width: 3, height: 3 };
    fb.blit_bgra(rect, &bgra_fill(rect, 0, 0xff, 0));

    assert_eq!(pixel(&fb, 3, 3), (0, 0xff, 0, 0xff));
    // The overhang must not have wrapped onto the start of any row.
    assert_eq!(pixel(&fb, 0, 3), (0, 0, 0, 0xff));
    assert_eq!(pixel(&fb, 0, 0), (0, 0, 0, 0xff));
}

#[test]
fn blit_tolerates_short_payloads() {
    let mut fb = Framebuffer::new(4, 4);
    let rect = Rect { x: 0, y: 0, width: 4, height: 4 };
    // Only one row of pixels for a four-row rect: stop, do not read past.
    let short = bgra_fill(Rect { x: 0, y: 0, width: 4, height: 1 }, 0, 0, 0xff);
    fb.blit_bgra(rect, &short);
    assert_eq!(pixel(&fb, 0, 0), (0xff, 0, 0, 0xff));
    assert_eq!(pixel(&fb, 0, 1), (0, 0, 0, 0xff));
}

#[test]
fn copy_rect_survives_overlapping_source_and_destination() {
    let mut fb = Framebuffer::new(4, 1);
    // Paint a distinct colour into each of the first two columns.
    for (x, r) in [(0u16, 0x11u8), (1, 0x22)] {
        let rect = Rect { x, y: 0, width: 1, height: 1 };
        fb.blit_bgra(rect, &bgra_fill(rect, 0, 0, r));
    }
    // Shift [0..2) one pixel right, so source and destination overlap at x=1.
    fb.copy_rect(
        Rect { x: 1, y: 0, width: 2, height: 1 },
        Rect { x: 0, y: 0, width: 2, height: 1 },
    );

    // Staging the source first is what keeps 0x11 from overwriting the 0x22
    // that the same pass still has to read.
    assert_eq!(pixel(&fb, 1, 0), (0x11, 0, 0, 0xff));
    assert_eq!(pixel(&fb, 2, 0), (0x22, 0, 0, 0xff));
}

#[test]
fn resize_reallocates_and_clears() {
    let mut fb = Framebuffer::new(2, 2);
    let rect = Rect { x: 0, y: 0, width: 2, height: 2 };
    fb.blit_bgra(rect, &bgra_fill(rect, 0, 0, 0xff));

    fb.resize(3, 1);
    assert_eq!((fb.width(), fb.height()), (3, 1));
    assert_eq!(fb.as_rgba().len(), 3 * BYTES_PER_PIXEL);
    assert_eq!(pixel(&fb, 0, 0), (0, 0, 0, 0xff));
}

#[test]
fn resize_to_the_same_size_keeps_contents() {
    let mut fb = Framebuffer::new(2, 2);
    let rect = Rect { x: 0, y: 0, width: 2, height: 2 };
    fb.blit_bgra(rect, &bgra_fill(rect, 0, 0, 0xff));
    fb.resize(2, 2);
    assert_eq!(pixel(&fb, 0, 0), (0xff, 0, 0, 0xff));
}

#[test]
fn a_region_is_extracted_tightly_packed() {
    let mut fb = Framebuffer::new(4, 4);
    let rect = Rect { x: 1, y: 1, width: 2, height: 2 };
    fb.blit_bgra(rect, &bgra_fill(rect, 0, 0, 0xff));

    let region = fb.region_rgba(rect);
    // Two by two pixels, with no stride from the wider surface left in.
    assert_eq!(region.len(), 2 * 2 * BYTES_PER_PIXEL);
    for pixel in region.chunks_exact(BYTES_PER_PIXEL) {
        assert_eq!(pixel, [0xff, 0, 0, 0xff], "every pixel in the region");
    }
}

#[test]
fn a_region_is_clipped_to_the_surface() {
    let fb = Framebuffer::new(4, 4);
    // Asking past the edge must yield what exists, not read out of bounds.
    let region = fb.region_rgba(Rect { x: 2, y: 2, width: 8, height: 8 });
    assert_eq!(region.len(), 2 * 2 * BYTES_PER_PIXEL);
}

/// The byte-at-a-time definition the word-wise swizzle must match exactly.
///
/// Kept as the scalar truth: the optimised path in the crate reads and writes
/// whole words, and this is what "correct" means for it.
fn scalar_truth_blit(width: u16, height: u16, rect: Rect, data: &[u8]) -> Vec<u8> {
    let mut pixels = vec![0u8; width as usize * height as usize * BYTES_PER_PIXEL];
    for chunk in pixels.chunks_exact_mut(BYTES_PER_PIXEL) {
        chunk[3] = 0xff;
    }
    for row in 0..rect.height as usize {
        let dst_y = rect.y as usize + row;
        if dst_y >= height as usize {
            break;
        }
        let visible = (rect.width as usize).min((width as usize).saturating_sub(rect.x as usize));
        for column in 0..visible {
            let s = (row * rect.width as usize + column) * BYTES_PER_PIXEL;
            let d = (dst_y * width as usize + rect.x as usize + column) * BYTES_PER_PIXEL;
            let Some(src) = data.get(s..s + BYTES_PER_PIXEL) else {
                return pixels;
            };
            pixels[d] = src[2];
            pixels[d + 1] = src[1];
            pixels[d + 2] = src[0];
            pixels[d + 3] = 0xff;
        }
    }
    pixels
}

#[test]
fn the_word_wise_swizzle_matches_the_scalar_definition() {
    // Widths either side of the four-byte word and of common vector lanes, so
    // a tail handled wrongly shows up rather than hiding behind a round size.
    for width in [1u16, 2, 3, 4, 5, 7, 8, 15, 16, 17, 31, 33, 64] {
        for height in [1u16, 2, 5] {
            let rect = Rect { x: 0, y: 0, width, height };
            // A pattern where every channel differs, so a swapped or dropped
            // lane cannot coincidentally compare equal.
            let data: Vec<u8> = (0..rect.byte_len()).map(|i| (i * 7 % 251) as u8).collect();

            let mut fb = Framebuffer::new(width, height);
            fb.blit_bgra(rect, &data);
            assert_eq!(
                fb.as_rgba(),
                scalar_truth_blit(width, height, rect, &data),
                "width {width} height {height}"
            );
        }
    }
}

#[test]
fn the_swizzle_matches_the_scalar_definition_at_an_offset() {
    // An offset rect exercises the clipped-row path, where source and
    // destination have different strides.
    let rect = Rect { x: 3, y: 1, width: 5, height: 3 };
    let data: Vec<u8> = (0..rect.byte_len()).map(|i| (i * 13 % 251) as u8).collect();

    let mut fb = Framebuffer::new(9, 6);
    fb.blit_bgra(rect, &data);
    assert_eq!(fb.as_rgba(), scalar_truth_blit(9, 6, rect, &data));
}
