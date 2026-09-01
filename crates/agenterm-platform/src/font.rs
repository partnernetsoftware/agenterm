//! OS-neutral native-font service.

use crate::CapabilityStatus;
pub use crate::contract::font::{
    FontDiscovery, FontError, FontFileCandidate, FontMetrics, FontRequest, OpaqueWindowHandle,
    RasterGlyph,
};
use crate::selected;

impl FontError {
    pub fn to_capability_status(self) -> CapabilityStatus {
        let message = match self {
            Self::Unsupported => "native font creation is unsupported on this platform",
            Self::Unavailable => "no system font candidate is available",
            Self::InvalidRequest => "the font request is invalid",
            Self::DeviceContextUnavailable => "the native font device context is unavailable",
            Self::CreateFailed => "native font creation failed",
            Self::MetricsFailed => "native font metrics could not be measured",
            Self::RasterFailed => "native font glyph rasterization failed",
            Self::GlyphTooLarge => "native font glyph exceeds the allocation bound",
        };
        match self {
            Self::Unsupported => CapabilityStatus::Unsupported {
                reason: "native-font-creation-unsupported".into(),
            },
            _ => CapabilityStatus::Failed {
                code: self.code().into(),
                message: message.to_owned(),
            },
        }
    }
}

pub fn candidates() -> &'static [FontFileCandidate] {
    selected::font::candidates()
}

/// Faces consulted only for glyphs the primary font lacks.
///
/// The primary candidates are fixed-pitch monospace faces for stable cell metrics.
/// Without coverage fallbacks, CJK and emoji can still render as blank cells
/// (cell width reserved, glyphs missing). These are never selected as the
/// primary face.
pub fn fallback_candidates() -> &'static [FontFileCandidate] {
    selected::font::fallback_candidates()
}

pub fn probe() -> FontDiscovery {
    selected::font::probe()
}

pub fn primary_family_name() -> Result<&'static str, FontError> {
    selected::font::primary_family_name()
}

pub fn primary_metrics(size_px: u16) -> Result<FontMetrics, FontError> {
    selected::font::primary_metrics(size_px)
}

/// Name of the face actually selected by the platform rasterizer.
pub fn rasterizer_name() -> Result<String, FontError> {
    selected::font::rasterizer_name()
}

/// What the terminal grid was actually built on, measured rather than
/// requested.
///
/// A font is chosen from a wish list, and on Windows the system substitutes
/// silently for anything missing — so the family a build asks for and the face
/// a machine renders with are different questions. This answers the second
/// one, which is the only one a user reporting "the font looks wrong" can help
/// with.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PrimaryFaceReport {
    /// The face the system resolved to, as it names itself — which may be a
    /// localized name rather than the family that was requested.
    pub face: String,
    pub cell_width: u32,
    pub cell_height: u32,
    /// `None` where the platform cannot measure a single character's advance.
    pub ascii_advance: Option<u32>,
    pub full_width_advance: Option<u32>,
}

impl PrimaryFaceReport {
    /// Whether a full-width character occupies exactly two cells.
    ///
    /// The invariant a character grid depends on. `None` means it could not be
    /// measured, which is not the same as false and must not be reported as
    /// though it were.
    #[must_use]
    pub fn full_width_is_double(&self) -> Option<bool> {
        match (self.ascii_advance, self.full_width_advance) {
            (Some(ascii), Some(full)) if ascii > 0 => Some(full == ascii * 2),
            _ => None,
        }
    }
}

/// Measures the face the grid is built on at `size_px`.
pub fn primary_face_report(size_px: u16) -> Result<PrimaryFaceReport, FontError> {
    selected::primary_face_report(size_px)
}

/// The report assembled from the portable metrics and the rasterizer's
/// name -- what every platform without a native face report answers.
/// `selected.rs` decides who answers; no `cfg` lives here.
// This facade remains compiled on Windows so the module has one neutral
// shape, but `selected.rs` chooses the native Windows report there.
#[allow(dead_code)]
pub(crate) fn portable_primary_face_report(size_px: u16) -> Result<PrimaryFaceReport, FontError> {
    let metrics = primary_metrics(size_px)?;
    Ok(PrimaryFaceReport {
        face: rasterizer_name()?,
        cell_width: crate::numeric::ceil_f32(metrics.cell_width).max(1.0) as u32,
        cell_height: crate::numeric::ceil_f32(metrics.cell_height).max(1.0) as u32,
        ascii_advance: None,
        full_width_advance: None,
    })
}

/// Rasterizes one Unicode scalar without exposing font files or native handles.
pub fn rasterize(ch: char, size_px: u16) -> Result<Option<RasterGlyph>, FontError> {
    selected::font::rasterize(ch, size_px)
}

pub fn capability_status() -> CapabilityStatus {
    selected::font::probe_capability()
        .map(|()| CapabilityStatus::Available)
        .unwrap_or_else(FontError::to_capability_status)
}

/// A selected-platform font resource with deterministic native cleanup.
#[derive(Debug)]
pub struct NativeFont {
    raw: isize,
    metrics: FontMetrics,
}

impl NativeFont {
    pub(crate) const fn new(raw: isize, metrics: FontMetrics) -> Self {
        Self { raw, metrics }
    }

    /// Returns an opaque resource identity for product-native renderer glue.
    /// It is deliberately not an SDK-specific `HFONT` or toolkit type.
    pub const fn raw_handle(&self) -> isize {
        self.raw
    }

    pub const fn metrics(&self) -> FontMetrics {
        self.metrics
    }
}

impl Drop for NativeFont {
    fn drop(&mut self) {
        selected::font::destroy_terminal_font(self.raw);
    }
}

pub fn create_terminal_font(
    window: OpaqueWindowHandle,
    request: FontRequest<'_>,
) -> Result<NativeFont, FontError> {
    let (raw, metrics) = selected::font::create_terminal_font(window, request)?;
    Ok(NativeFont::new(raw, metrics))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coverage_fallbacks_exist_and_are_distinct_from_the_primary_face() {
        // A terminal on a CJK system renders blank cells if this list is
        // empty, so its emptiness is a real regression, not a style nit.
        let fallbacks = super::fallback_candidates();
        assert!(
            !fallbacks.is_empty(),
            "every platform needs coverage fallbacks"
        );

        // The primary face must stay a monospace Latin font: cell metrics come
        // from it, so a proportional CJK face must never lead the list.
        let primary = super::candidates();
        assert!(!primary.is_empty());
        assert_ne!(
            primary.first().map(|c| c.name),
            fallbacks.first().map(|c| c.name),
            "a coverage font must not become the primary face"
        );
    }

    #[test]
    fn failures_map_to_stable_typed_statuses() {
        assert!(matches!(
            FontError::MetricsFailed.to_capability_status(),
            CapabilityStatus::Failed {
                code,
                message
            } if code == "font_metrics_failed" && !message.is_empty()
        ));
        assert!(matches!(
            FontError::Unsupported.to_capability_status(),
            CapabilityStatus::Unsupported { .. }
        ));
    }
}
