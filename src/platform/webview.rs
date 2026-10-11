//! AgenTerm compatibility projection for passive system-WebView discovery.

pub(crate) use minicon_platform::webview::RuntimePresence;

pub(crate) fn probe_system_webview() -> minicon_platform::webview::SystemWebViewProbe {
    minicon_platform::webview::probe_system_webview()
}
