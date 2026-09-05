fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }

    if std::env::var_os("CARGO_FEATURE_PROCESS").is_some() {
        for source in ["native/process_inspection.c", "native/process_inspection.h"] {
            println!("cargo:rerun-if-changed={source}");
        }
        cc::Build::new()
            .file("native/process_inspection.c")
            .flag("-Wall")
            .compile("agenterm_platform_process_inspection");
    }

    if std::env::var_os("CARGO_FEATURE_DEVICE_CAPTURE").is_none() {
        return;
    }

    for source in [
        "native/device_capture.m",
        "native/device_capture.h",
        "native/usbmux_probe.m",
        "native/usbmux_probe.h",
    ] {
        println!("cargo:rerun-if-changed={source}");
    }

    cc::Build::new()
        .file("native/device_capture.m")
        .file("native/usbmux_probe.m")
        .flag("-fobjc-arc")
        .flag("-Wall")
        .compile("agenterm_platform_device_capture");

    for framework in [
        "Foundation",
        "AVFoundation",
        "CoreMedia",
        "CoreMediaIO",
        "CoreImage",
        "CoreVideo",
        "CoreGraphics",
    ] {
        println!("cargo:rustc-link-lib=framework={framework}");
    }
}
