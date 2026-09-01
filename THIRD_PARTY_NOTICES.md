# Third-party notices

AgenTerm uses the following direct Rust dependencies. Their exact resolved
versions and transitive dependency graph are recorded in `Cargo.lock` and the
generated `agenterm-sbom.spdx.json`.
Copyright and license terms remain with their respective authors.

| Package | Declared license |
| --- | --- |
| `ab_glyph` | Apache-2.0 |
| `aes` | MIT OR Apache-2.0 |
| `cipher` | MIT OR Apache-2.0 |
| `anyhow` | MIT OR Apache-2.0 |
| `atspi` | Apache-2.0 OR MIT |
| `async_io_stream` | Unlicense |
| `cc` (dev dependency) | MIT OR Apache-2.0 |
| `flate2` | MIT OR Apache-2.0 |
| `futures` | MIT OR Apache-2.0 |
| `jpeg-decoder` | MIT OR Apache-2.0 |
| `libc` | MIT OR Apache-2.0 |
| `libloading` | ISC |
| `md-5` | MIT OR Apache-2.0 |
| `mlua` | MIT |
| `num-bigint` | MIT OR Apache-2.0 |
| `objc2` | MIT |
| `objc2-app-kit` | MIT |
| `objc2-foundation` | MIT |
| `object` | Apache-2.0 OR MIT |
| `png` | MIT OR Apache-2.0 |
| `rand` | MIT OR Apache-2.0 |
| `rusqlite` | MIT |
| `serde` | MIT OR Apache-2.0 |
| `serde_json` | MIT OR Apache-2.0 |
| `sha2` | MIT OR Apache-2.0 |
| `softbuffer` | MIT OR Apache-2.0 |
| `sqlparser` | Apache-2.0 |
| `tauri` | MIT OR Apache-2.0 |
| `tauri-build` (build dependency) | Apache-2.0 OR MIT |
| `tinyvm` | MIT OR Apache-2.0 |
| `tinyvm-qjs` | MIT OR Apache-2.0 |
| `tokio` | MIT |
| `tempfile` | MIT OR Apache-2.0 |
| `thiserror` | MIT OR Apache-2.0 |
| `tokio-stream` | MIT |
| `tokio-util` | MIT |
| `tracing` | MIT |
| `unicode-width` | MIT OR Apache-2.0 |
| `ureq` | MIT OR Apache-2.0 |
| `vt100` | MIT |
| `walkdir` | Unlicense/MIT |
| `wat` | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT |
| `windows-sys` | MIT OR Apache-2.0 |
| `winit` | Apache-2.0 |
| `winresource` (build dependency) | MIT |
| `x11rb` | MIT OR Apache-2.0 |
| `zbus` | MIT |

The corresponding sources and complete license files are available from each
package's Cargo registry entry or declared Git source.
`scripts/qjs/supply-chain.qjs` uses
`cargo metadata --locked` as the authoritative inventory, requires this table
to cover every direct dependency, rejects unreviewed license expressions, and
records every resolved transitive package in the SPDX inventory.
