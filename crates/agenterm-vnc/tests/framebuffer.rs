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
