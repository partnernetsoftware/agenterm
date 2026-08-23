// The webview owns the entire UI, so a console window on Windows would only
// ever be an empty artifact of the launch.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    agenterm_vnc_app_lib::run();
}
