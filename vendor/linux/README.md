# Bundled Linux GUI libraries

`libxkbcommon-x11.so.0` is vendored here because winit loads it through
`xkbcommon-dl` at runtime. Slim X11 desktops often ship `libxkbcommon0` without
the X11 bridge package (`libxkbcommon-x11-0` on Debian/Ubuntu). AgenTerm stages
these bytes beside the process when the host does not already provide the SONAME.
The bridge itself needs `libxcb-xkb.so.1`, which is also absent on that minimal
image, so the matching Ubuntu `libxcb-xkb1` runtime library is staged with it.

Source: Ubuntu 24.04 `libxkbcommon-x11-0` 1.6.0-1build1 (`libxkbcommon-x11.so.0.0.0`).
        Ubuntu 24.04 `libxcb-xkb1` 1.15-1ubuntu2 (`libxcb-xkb.so.1.0.0`).
