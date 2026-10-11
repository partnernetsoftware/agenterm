//! AgenTerm compatibility facade over `minicon-platform` screenshot encoding.

use crate::platform::contract::ui_screenshot::{UiScreenshotError, XrgbFrame};

pub(crate) fn write_xrgb_png(frame: XrgbFrame<'_>) -> Result<(), UiScreenshotError> {
    minicon_platform::screenshot::write_xrgb_png(frame).map(|_| ())
}
