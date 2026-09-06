//! Shared file-font rasterizer for Unix adapters.

use std::{fs::File, sync::OnceLock};

use ab_glyph::{Font, FontRef, GlyphId, PxScale, ScaleFont};

use crate::contract::font::{FontError, FontFileCandidate, RasterGlyph};

const MAX_COLLECTION_FACES: u32 = 32;
const MAX_GLYPH_DIM: u32 = 4096;

// FontRef borrows only for a raster call. The owner keeps the file mapping,
// never a forged static slice or a heap copy of an entire font collection.
struct FontFile {
    name: &'static str,
    data: memmap2::Mmap,
}

impl FontFile {
    fn open(candidate: FontFileCandidate) -> Option<Self> {
        let file = File::open(candidate.absolute_path()).ok()?;
        // SAFETY: candidates are adapter-owned installed system fonts, not
        // caller-supplied writable buffers. They must not be modified/truncated
        // in place while mapped (normal package replacement renames the inode).
        // Mmap owns/unmaps the region; FontRef cannot outlive its borrowed data.
        let data = unsafe { memmap2::Mmap::map(&file) }.ok()?;
        FontRef::try_from_slice(&data).ok()?;
        Some(Self {
            name: candidate.name,
            data,
        })
    }

    fn face_for(&self, ch: char) -> Option<FontRef<'_>> {
        for index in 0..MAX_COLLECTION_FACES {
            let Ok(font) = FontRef::try_from_slice_and_index(&self.data, index) else {
                break;
            };
            if font.glyph_id(ch) != GlyphId(0) {
                return Some(font);
            }
        }
        None
    }
}

struct LazyFontFile {
    candidate: FontFileCandidate,
    // Cache failed opens too: a missing font must not cause I/O on every glyph.
    file: OnceLock<Option<FontFile>>,
}

struct Renderer {
    primary: Option<FontFile>,
    fallback: Vec<LazyFontFile>,
}

impl Renderer {
    fn new(primary: &[FontFileCandidate], fallback: &[FontFileCandidate]) -> Self {
        Self {
            primary: primary.iter().copied().find_map(FontFile::open),
            fallback: fallback
                .iter()
                .copied()
                .map(|candidate| LazyFontFile {
                    candidate,
                    file: OnceLock::new(),
                })
                .collect(),
        }
    }

    fn face_for(&self, ch: char) -> Option<FontRef<'_>> {
        if let Some(font) = self.primary.as_ref().and_then(|file| file.face_for(ch)) {
            return Some(font);
        }
        for fallback in &self.fallback {
            if let Some(font) = fallback
                .file
                .get_or_init(|| FontFile::open(fallback.candidate))
                .as_ref()
                .and_then(|file| file.face_for(ch))
            {
                return Some(font);
            }
        }
        None
    }
}

type CandidateSource = fn() -> &'static [FontFileCandidate];

fn renderer(primary: CandidateSource, fallback: CandidateSource) -> &'static Renderer {
    static RENDERER: OnceLock<Renderer> = OnceLock::new();
    RENDERER.get_or_init(|| Renderer::new(primary(), fallback()))
}

pub(crate) fn rasterizer_name(
    primary: CandidateSource,
    fallback: CandidateSource,
) -> Result<String, FontError> {
    renderer(primary, fallback)
        .primary
        .as_ref()
        .map(|face| face.name.to_owned())
        .ok_or(FontError::Unavailable)
}

pub(crate) fn rasterize(
    primary: CandidateSource,
    fallback: CandidateSource,
    ch: char,
    size_px: u16,
) -> Result<Option<RasterGlyph>, FontError> {
    let size_px = size_px.clamp(8, 72);
    let renderer = renderer(primary, fallback);
    let Some(font) = renderer.face_for(ch) else {
        return Ok(None);
    };
    let scaled = font.as_scaled(f32::from(size_px));
    let glyph_id = scaled.glyph_id(ch);
    let Some(outlined) =
        scaled.outline_glyph(glyph_id.with_scale(PxScale::from(f32::from(size_px))))
    else {
        return Ok(None);
    };
    let bounds = outlined.px_bounds();
    let width = bounded_dimension(bounds.width())?;
    let height = bounded_dimension(bounds.height())?;
    let len = (width as usize)
        .checked_mul(height as usize)
        .ok_or(FontError::GlyphTooLarge)?;
    let mut alpha = vec![0u8; len];
    outlined.draw(|x, y, coverage| {
        if x < width && y < height {
            alpha[(y * width + x) as usize] = (coverage.clamp(0.0, 1.0) * 255.0).round() as u8;
        }
    });
    Ok(Some(RasterGlyph {
        alpha,
        width,
        height,
        offset_x: bounds.min.x.round() as i32,
        offset_y: (scaled.ascent() + bounds.min.y).round() as i32,
    }))
}

fn bounded_dimension(value: f32) -> Result<u32, FontError> {
    if !value.is_finite() || value < 0.0 || value > MAX_GLYPH_DIM as f32 {
        return Err(FontError::GlyphTooLarge);
    }
    Ok(value as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "macos")]
    #[test]
    fn ascii_and_cjk_do_not_preload_emoji() {
        use crate::selected::font;
        let renderer = Renderer::new(font::candidates(), font::fallback_candidates());
        assert!(renderer.primary.is_some(), "requires installed macOS fonts");
        for ch in ' '..='~' {
            assert!(renderer.face_for(ch).is_some(), "missing ASCII {ch:?}");
        }
        assert!(
            renderer
                .fallback
                .iter()
                .all(|file| file.file.get().is_none())
        );
        let cjk = renderer.face_for('中').expect("installed CJK fallback");
        let outline = cjk
            .outline_glyph(cjk.glyph_id('中').with_scale(16.0))
            .expect("CJK must retain an outline");
        let mut covered = false;
        outline.draw(|_, _, coverage| covered |= coverage > 0.0);
        assert!(covered, "CJK must retain visible pixels");
        let emoji = renderer
            .fallback
            .iter()
            .find(|file| file.candidate.name == "Apple Color Emoji")
            .unwrap();
        assert!(emoji.file.get().is_none(), "CJK must stop before emoji");
        // Exhausting the search may open emoji metadata, but never copies its
        // bitmap tables to the heap. Repeated misses reuse the same mapping.
        assert!(renderer.face_for('\u{10ffff}').is_none());
        assert!(emoji.file.get().is_some());
        assert!(renderer.face_for('\u{10ffff}').is_none());
    }

    #[test]
    fn unavailable_fallback_is_cached_without_a_primary() {
        let renderer = Renderer::new(
            &[],
            &[FontFileCandidate {
                name: "missing",
                components: &["__agenterm_missing_font_fixture__", "missing.ttf"],
            }],
        );
        assert!(renderer.face_for('A').is_none());
        assert!(matches!(renderer.fallback[0].file.get(), Some(None)));
        assert!(renderer.face_for('B').is_none());
    }

    #[test]
    fn glyph_dimensions_reject_non_finite_and_oversized_values() {
        assert_eq!(bounded_dimension(42.0), Ok(42));
        assert_eq!(bounded_dimension(f32::NAN), Err(FontError::GlyphTooLarge));
        assert_eq!(
            bounded_dimension(1_000_000.0),
            Err(FontError::GlyphTooLarge)
        );
    }
}
