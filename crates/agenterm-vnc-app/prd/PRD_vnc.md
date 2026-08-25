# AgenTerm VNC — product and protocol notes

A VNC client: `agenterm-vnc` owns the RFB session and pixels, `agenterm-vnc-app`
is the Tauri shell. The split exists because the same session surface is meant
to carry RDP and SSH backends later, and eventually an AgenTerm-native
transport once the product serves remote desktops as well as consuming them.

## Memory palace: what was learned the hard way

Each entry is a conclusion that cost real debugging. The measurement matters
more than the claim; where a number appears, it was taken against a real macOS
Screen Sharing server at 3840x2160, not a mock.

### The server answers the encodings you ask for, and nothing more

For a long time this client looked hopeless on macOS: a cursor move drew rects
like 3792x1816, twenty-seven megabytes of pixels for a few that changed, and
the conclusion drawn was "RFB gives a client no way to ask for less." That was
wrong. It was answering the encodings that had been declared.

Same full-screen update, varying only `SetEncodings`:

| declared | bytes on the wire |
| --- | --- |
| Raw (0) | 33.18 MB |
| Tight (7) | 33.18 MB — **the server does not implement it** and silently falls back |
| ZRLE (16) | 3.84 MB |
| Apple 1000 | 0.13 MB |
| Apple 1001 | 0.42 MB |
| Apple 1002 | 1.74 MB |
| Apple 1100-1105 | 33.18 MB (falls back) |

Two things follow. Requesting Tight, which this client did by default, bought
nothing at all against macOS. And "the protocol is the limit" is a conclusion
that must never be reached without varying the encoding list first.

### Apple's client is ordinary RFB; its speed is an encoding choice

`ScreenSharing.framework` exports `_RFBAuthenticate`, `_RFBPostMouseEvent`,
`_RFBSetViewerEncodings`, `_RFBGetViewerEncodings` and five quality presets
(`_SSLowQualityEncodings` through `_SSFullQualityEncodings`). It is the same
protocol this crate speaks. It also exports `_kSSVideoEncoding_AVCMediaStream`
and links AVConference and VideoToolbox, but that H.264 path is a *separate*
mode, not what a plain `vnc://` connection uses.

The registry at rfbproto lists 1000-1002, 1011 and 1100-1105 as reserved to
Apple Inc. Nothing documents their wire format, and no open implementation
consulted -- LibVNCServer, TigerVNC, noVNC, gtk-vnc, vncdotool, Wireshark's
dissector -- supports them. But the negotiation is open: any client that
declares 1000 gets 1000.

### The Apple encodings decoded

Probed directly. All three are a rect header, a four byte big-endian length,
then a **zlib stream that persists across messages** -- one `Decompressobj` for
the whole session, like Tight's streams, not one per rect.

| encoding | decompressed | bits per pixel | symbol it matches |
| --- | --- | --- | --- |
| 1000 | 1,036,800 | 1 | `_kSSVideoEncoding_SubZlibHalftone` |
| 1001 | 4,147,200 | 4 | `_kSSVideoEncoding_SubZlib16Gray` |
| 1002 | 16,588,800 | 16 | `_kSSVideoEncoding_SubZlibThousandsCodec` |

For a 3840x2160 screen those are exactly `W*H/8`, `W*H/2` and `W*H*2`, so the
payload is a plain raster at a fixed depth -- the compression is entirely
zlib's, plus the depth reduction. 1000 is a black-and-white halftone, which is
why 0.13 MB is not as good as it sounds.

**1002 is the one worth having:** full colour, and 1.74 MB against ZRLE's 3.84.

Layout is row-major with stride equal to the screen width; a rendered dump is
structurally correct -- windows, Dock and Finder icons all legible -- so this
much is settled. The channel order within each 16-bit pixel is **not** yet:
RGB565, BGR565 and RGB555 in both byte orders all render recognisable geometry
with wrong colours. Three reference pixels, little-endian: (0,0) is `0xd615`, the middle of the
menu bar is `0x9511`, screen centre is `0x9d77`. Read as RGB565 those are
(213,194,172), (148,161,139) and (156,174,189) -- structurally right but tinted
green throughout, and no menu bar is that colour. So the remaining error is
probably not channel order alone: candidates are a palette or colour map the
server sent that this probe ignored, a gamma or colour-space conversion, or a
per-row filter like PNG's. Settling it needs a frame whose true colours are
known -- a solid-colour desktop would do -- or a capture of Apple's own client
decoding the same bytes.

### Frame delivery, and what the measurements actually said

- **The IPC, not the canvas, was the bottleneck.** Fetching a frame took
  560 ms against 13 ms to draw it, because a Tauri command's reply crosses as
  JSON on the postMessage path and a `Vec<u8>` in JSON is an array of decimal
  numbers. Serving frames from a registered URI scheme instead took the fetch
  to 9.9 ms. Two things silently break that path: the scheme must be allowed in
  `connect-src`, and the response needs `Access-Control-Allow-Origin`.
- **Do not merge dirty regions by adjacency.** macOS tiles updates into 64x64
  rects and every tile touches its neighbours, so any "merge when they touch"
  rule chains a screen's worth into one full-screen send. Measured: merging
  gave 5 fps and 13,601 KB a frame, not merging gave 18 fps and 16 KB.
- **Carry a whole update's tiles in one frame.** A frame per rect meant two
  thousand IPC round trips for one repaint, and the webview could not drain
  them; the window rendered as scattered tiles on black.
- **Overlap requests with the server's encoding.** The server spends 37 to
  453 ms before the first rect of an answer arrives. Allowing four outstanding
  requests took a continuous drag from 0.6 fps to 1.7.
- **Sixteen-bit colour is not automatically cheaper.** It measured *worse*
  (44 MB/s against 22) because it disqualifies Tight -- though against a server
  that never implemented Tight, that particular trade-off does not apply.

### Failure modes that produced no error message

- An unknown encoding id decoded as Raw pixels. Anything the client could not
  name was painted from whatever the compressed stream contained, and left the
  reader misaligned for every rect after it: a screen of scrambled tiles, no
  log line. Unknown encodings must be an error.
- `vnc-rs` built two enums out of untrusted bytes with `transmute`, which is
  undefined behaviour for any unlisted value and aborted the process with a
  non-unwinding panic nothing could catch.
- macOS announces `RFB 003.889`, which does not fit the `u8` the minor version
  was parsed into. Falling back to 3.3 sent the handshake down the branch where
  the server dictates the security type, so the client never chose one and the
  connection simply hung.
- A frontend guard tested `byteLength` on a value that was sometimes a plain
  Array, where it is `undefined`, so every frame was silently rejected and the
  window stayed black.

### Method, restated because it kept being violated

Mock servers only ever confirm what was already imagined. Every serious defect
here -- the JPEG rects, the private encodings, the tiling, the IPC cost --
survived a passing test suite and was found only by connecting to a real
server. Treat "it works against my test server" as unverified.

## Current state

Works against macOS Screen Sharing: ARD authentication, ZRLE and Raw decoding,
correct rendering, roughly 1.7 fps in a continuous drag at 3840x2160.

The next substantial win is encoding 1002, which needs its channel order
settled. Until then the client is asking for ZRLE, at more than twice the
bytes, from a server that would happily send less.
