<img src="assets/icon.png" width="96" height="96" alt="fracst icon">

# fracst

A fast deep-zoom Mandelbrot set explorer written in modern Rust, with
automatic iteration limits and minibrot-aware references.

- Zoom to 10^1000 and beyond, interactively. Zooms glide into place, and
  frames render in the background while the last one stays on screen.
- A dark, frameless window where the image fills the screen, with a floating
  toolbar and a settings card you can hide with Tab.
- **Ctrl+K** searches everything the app does: actions, every setting,
  recent locations and video projects.
- Opens and saves `.kfr` location files, and embeds the location in saved
  PNGs so images can be reopened.
- Palette colouring: 1024-entry palettes interpolated from key colours,
  `IterDiv`, `ColorOffset`, transfer functions, and neighbour-difference slope shading.
- Newton's method minibrot finder ("jump to the nearest minibrot").
- Zoom videos from the app or the CLI: keyframes rendered here or on a remote
  GPU (resumable), smooth interpolation, a speed curve, animated colours and
  audio, encoded with ffmpeg (which fracst can download for you).
- Rendering on a remote NVIDIA GPU (`fracst-server`, with a CUDA engine that
  gives bit-identical frames), self-hosted or on Modal or fal.ai, and
  shareable through a gateway with per-person keys.
- CLI for batch renders and supersampling.

![fracst exploring a spiral at zoom 2.2e238, with the settings card open](assets/screenshot.png)

| | |
|---|---|
| ![A minibrot at zoom 1.5e400, with the settings card hidden](assets/screenshots/deep-minibrot.jpg) | ![The same minibrot with its render statistics: 22.9% interior, 208× fewer steps than iterations](assets/screenshots/statistics.jpg) |
| A minibrot at zoom 1.5e400, with the settings hidden (Tab). | Its statistics: a 4M iteration limit found automatically, the reference on the minibrot's nucleus, 208 iterations skipped per step. |
| ![The Ctrl+K palette filtered to "palette"](assets/screenshots/search.jpg) | ![The Make video window](assets/screenshots/video.jpg) |
| **Ctrl+K**: switches and choices change in place. | **Make video…**: a zoom video into the current location. |

## Download

Installers for **Windows** (`-setup.exe`), **macOS** (`.dmg`, universal: Apple
Silicon and Intel) and **Linux** (`.deb`, or a tarball with `install.sh` for
other distributions) are on the [Releases page](https://github.com/alexcrisan1611/fracst/releases/latest),
along with portable zips. They install the explorer (`fracst`) and the
command-line renderer (`fracst-cli`). The builds are not code-signed. On Windows
choose **More info → Run anyway** if SmartScreen asks. On macOS right-click the
app and choose **Open** the first time.

At startup the app asks GitHub for the latest release's version and, if it is
newer, shows a link to it in the toolbar. Nothing else is sent. Turn this off
under **App → Check for updates at startup** in the settings (or set
`FRACST_NO_UPDATE_CHECK=1`).
To upgrade, run the new installer over the old one; settings are kept.

To build from source instead, install [Rust](https://rustup.rs) and run
`cargo run --release -p fracst`.

## How it works

fracst uses perturbation theory. It computes one reference orbit in high
precision, then iterates every pixel as a small offset from it in fast
hardware arithmetic. On top of that:

- **Bilinear approximation (BLA)** skips iterations anywhere along the orbit,
  in steps of up to 2^k.
- **Rebasing.** When a pixel's orbit comes closer to zero than to the
  reference, the pixel restarts against the beginning of the reference. One
  reference serves the whole image, and glitches can't happen.
- **Nucleus-snapped reference.** When a minibrot is near the view, the
  reference moves onto its nucleus (found with Newton's method), so its orbit
  stops after one period.
- **Automatic iteration limits**, predicted from nearby minibrots and
  corrected per frame (see below).
- **Interior detection.** With a periodic reference it checks the cycle
  multiplier at the end of each period, which stays reliable deep inside
  minibrots. Otherwise it uses the derivative with respect to z₁.
- **Extended range only where it's needed.** Shared-exponent complex
  numbers are used while a pixel's offset is below 2^-480, and plain `f64`
  after that. In extended range the orbit is precomputed in that format,
  and escape and rebase tests are skipped wherever they provably can't
  fire.
- **SIMD and FMA.** On CPUs with AVX2, four `f64` pixels advance together.
  A pixel that needs a decision (escape, rebase, a possible BLA skip, an
  interior check) takes that step through the scalar loop, so images are
  bit-identical. A per-frame gauge switches SIMD off where those events
  would cost more than they save. Fused multiply-adds are chosen at
  runtime.
- **Reference arithmetic** uses a custom fixed-point bignum: squarings only,
  with truncated products.

BLA and rebasing are due to Zhuoran. [docs/performance.md](docs/performance.md)
records which speed-ups were tried, how they were measured, and which were
kept.

## Automatic iterations

A fixed iteration limit is either too low near minibrots, so their surroundings
render as solid interior, or wastefully high elsewhere. With **Auto iterations** (on by default in the
explorer, `--auto-iterations` in the CLI) fracst picks the limit for each frame:

1. **Predict.** While computing the reference orbit, a ball test finds the
   lowest-period minibrot within a few view radii. Pixels around it escape at
   multiples of its period P, so the limit starts at 8·P or more. Without one,
   it starts from the previous frame's suggestion, but never below a floor that
   grows gently with zoom.
2. **Correct.** Pixels that reach the limit without escaping, and without being
   detected as interior, are *undecided*. Suppose many pixels escaped just under
   the limit, or undecided pixels make up more than a thin rim around the
   interior. Then the limit is raised ×4 and only the undecided pixels are
   rendered again. A round that resolves fewer than half of them ends the
   escalation, because what remains converges too slowly to be worth chasing.
3. **Suggest.** The next frame starts from twice the escape time of all but the
   slowest 0.01% of pixels. A few stragglers next to the boundary therefore
   cannot ratchet the limit up, and it falls again when a view needs less.

Two more things help. When a minibrot is near, the reference moves onto its
nucleus (found with Newton's method). Its orbit then stops after one period.
Interior detection measures the cycle multiplier at each period end, which is
reliable even for deep minibrots.

## Benchmarks

1920×1080, AMD Ryzen 7 8845HS (8 cores / 16 threads), `cargo run --release
-p fracst-core --example bench -- --baseline`, v0.4.1. Times are the first
render including the reference orbit, best of two runs. They vary by about
±30% between runs on this laptop (boost clocks and thermals).

| Location | Zoom | Avg iterations | fracst | Plain perturbation | Speedup |
|---|---|---:|---:|---:|---:|
| Overview | 1 | 11 | 0.03 s | 0.05 s | 2× |
| Seahorse minibrot | 3e8 | 3,243 | 1.7 s | 1.1 s | 0.65× |
| Spiral | 2e238 | 2,186 | 0.08 s | 0.76 s | 10× |
| Spiral | 8e397 | 4,025 | 0.14 s | 10.4 s | 75× |
| Spiral | 1e998 | 11,764 | 0.19 s | 30.3 s | 160× |
| Inside a minibrot (21% interior, 1M max iterations) | 2e400 | 20,830 | 0.40 s | not run | — |

"Plain perturbation" is the same engine with BLA and interior detection
turned off (one reference with rebasing, still with SIMD and FMA). It shows
what the iteration skipping buys, and is not a comparison with another
program. At shallow zooms such as the seahorse row, BLA cannot skip
anything (3,242 steps for 3,243 iterations), so the BLA lookups and interior
checks are pure overhead, and fracst is slower than plain perturbation there.

`--export` writes every benchmark location to `bench-locations/*.kfr`, so
you can render the same views again with other settings or later versions.

## Accuracy

`cargo test --release` renders test views at zooms from 1e6 to about 1e400:
shallow and deep minibrots, spirals, and a reference sitting exactly on a nucleus.
It compares sampled pixels against brute-force fixed-point iteration of
the same points at higher precision. The renders must agree on the escape
iteration, and interior detection may not flag any pixel that escapes. All
of these run with and without BLA.

The BLA tolerance defaults to ε = 2^-40. A looser 2^-24 misplaced more
than 1% of pixels near a minibrot in the tests. The tighter tolerance costs
only a few percent more steps. You can change it in
the GUI (Rendering → BLA precision bits) or with `--bla-bits`.

## Usage

### Explorer (`fracst`)

```
cargo run --release -p fracst [location.kfr | image.png]
```

| Action | Effect |
|---|---|
| Left click | Centre on the point and zoom in (×4 by default) |
| Right click | Zoom out |
| Mouse wheel | Zoom around the cursor |
| Drag | Pan |
| Esc | Cancel the current render |
| Tab | Show or hide the settings |
| Ctrl+K | Search everything the app does: actions, every setting, video commands |
| Ctrl+O / Ctrl+S / Ctrl+E | Open a location or video project, save `.kfr`, export PNG |
| Drop a file | Open a `.kfr`, a PNG with an embedded location, or a video project folder |

On a Mac, use Cmd instead of Ctrl.

The image fills the window. A floating toolbar at the top left holds
**Open**, **Save**, **Export**, the **Video** menu, **Search**, the settings
switch (⚙, or Tab) and the controls (**?**). Drag the wordmark to move the
window. The bar at the bottom shows the zoom, the iteration limit, render
progress and messages.

The settings card on the right has these sections:

- **Location**: arbitrary-precision coordinates, zoom and iteration limit,
  **Go**, **Reset**, and **Copy** and **Paste…** as `.kfr` text.
- **Navigation**: the click zoom (×2 to ×32), **Zoom out**, the zoom
  animation, **Find minibrot** with how large to frame it, and **Auto
  iterations**.
- **Colour**: palette presets, a random palette, editable key colours,
  `IterDiv`, offset, transfer function, smooth or flat colouring, interior
  colour, and slope shading (power, ratio, angle).
- **Rendering**: resolution scale, **Render only a region** (a small centred
  part of the view, for slow deep locations), BLA and its precision, bailout,
  SIMD, interior detection, reference snapping and reuse, and the direction
  of the imaginary axis.
- **Remote GPU**: render on a `fracst-server`, such as a cloud GPU (see below).
- **Export**: a PNG of any size with supersampling, and **Make video…** (see below).
- **Statistics**: the frame's timings by stage, the iteration limit and how
  many passes it took, the nearby minibrot's period, the reference orbit,
  precision, BLA skip factor, iteration range, and the share of interior pixels.
- **App**: the startup update check, the controls, and the version.

**Ctrl+K** (or **Search** in the toolbar) lists everything fracst can do.
Type a few letters of a name: actions run at once, switches and choices such
as palettes, transfer functions or the click zoom change right there, and
anything with a value opens its place in the settings and highlights it.
Entries used lately come first, and the last ten locations opened or saved
and video projects used are there too.

Zooms glide into place (**Navigation → Zoom animation**: Quick, Smooth, Slow
or Off), and jumps such as opening a file or finding a minibrot cross-fade.
While a frame renders, the previous one is shown stretched to the new view.
While the mouse wheel turns, the image is only scaled, and the next frame
starts once the wheel stops. Slow frames get a quarter-resolution preview first.

### Command line (`fracst-cli`)

```
# Render a location (or a PNG with an embedded location) at 1080p with 2x2 supersampling
fracst-cli location.kfr -W 1920 -H 1080 --supersample 2 -o out.png

# Coordinates directly, any number of digits
fracst-cli --re -0.7436438870371587 --im 0.1318259042053120 --zoom 1e12 -i 50000 -o seahorse.png

# Jump to the nearest minibrot and frame it at 1/4 of the image height
fracst-cli location.kfr --find-minibrot --mini-zoom 0.25 --save-kfr -o mini.png

# Zoom-out PNG sequence (reuses the reference orbit throughout)
fracst-cli deep.kfr --zoom-out 500 --zoom-out-factor 2 -o frames/zoom.png
```

`fracst-cli --help` lists every option.

### Zoom videos

**Video → Make video…** in the toolbar (or under Export, or Ctrl+K) sets up a
video from the whole set into the current location. **Video → Open video
project…** reopens one, and opening it moves the explorer to its location. fracst renders one *keyframe* per
doubling of the zoom, from the location outwards, here or on the remote GPU,
in the background while you keep exploring. Each keyframe is saved to a
project folder as soon as it is done, so a job can pause, crash or be cut off
by a gateway's usage limit and resume where it stopped. **Estimate** renders
the deepest (slowest) keyframe first and estimates the rest.

The keyframes keep raw iteration data, so the colours are applied when the
video is encoded: change the palette, add palette cycling or colour keys
(colours at moments of the video, eased in between), change the length or the
speed curve, add music, and encode again without rendering anything. A
preview slider shows any moment before encoding. Each frame stacks the
keyframes around its zoom, deepest on top with soft edges, so the zoom is
smooth and the centre stays sharp.

Encoding uses ffmpeg (H.264, H.265, AV1 or ProRes). If none is found, fracst
offers to download a pinned, checksum-verified static build into its data
folder; without one it writes PNG frames instead.

```
# A 1080p60 video, one zoom doubling per second at full speed
fracst-cli video deep.kfr -W 1920 -H 1080 --fps 60 --per-doubling 1 -o deep.mp4

# As long as the music, keyframes on a remote GPU, colours cycling
fracst-cli video deep.kfr --audio song.flac --fit-audio --cycle 30 --remote wss://…/stream

# Resume a paused project, or re-encode it with other colours
fracst-cli video --project deep-video --colour-key "0:iter_div=0.02" --colour-key "60:iter_div=0.5"
```

See [docs/video.md](docs/video.md) for how it works and every setting.
`--zoom-out N` still writes a plain PNG sequence of N frames.

### Remote GPU (`fracst-server`)

`fracst-server` renders frames for the app and `fracst-cli --remote` over a
WebSocket. On a machine with an NVIDIA GPU, it iterates the pixels with a
CUDA kernel that produces bit-identical frames. The app keeps colouring and
the interface local, so only views and finished frames cross the network.
`deploy/modal/` and `deploy/fal/` deploy it on Modal or fal.ai in one
command, and `gateway/` (a Cloudflare Worker) shares it with others through
per-person fracst keys with usage limits, keeping your GPU token secret. See
[docs/remote.md](docs/remote.md) for setup, costs and how it is tested.

```
fracst-server --listen 0.0.0.0:7878 --token SECRET     # on the GPU machine
fracst-cli --remote ws://gpu-box:7878 --key SECRET deep.kfr -W 3840 -H 2160 -o deep.png
```

### Benchmarks

```
cargo run --release -p fracst-core --example bench [-- --baseline] [--export]
```

## Location files

Locations are stored as `.kfr` text, one `Key: value` per line. Read and
written: `Re`, `Im`, `Zoom` (any exponent), `Iterations`, `IterDiv`,
`ColorOffset`, `ColorMethod` (0 linear, 1 sqrt, 2 cbrt, 3 log, 9 log-log, 11 fourth root),
`Colors` and `InteriorColor` (B,G,R order), `Smooth`, `Flat`, `Slopes`,
`SlopePower`, `SlopeRatio`, `SlopeAngle`, and `ImagPointsUp`. In these files
the imaginary axis points down unless `ImagPointsUp: 1` is set. fracst
follows that, and you can toggle it under Rendering → Imaginary axis points down.
Saved PNGs carry the same text in a `Comment` chunk.

Not supported yet:

- Formulas other than z² + c (`FractalType` ≠ 0, `Power` ≠ 2).
- Rotation and stretch.
- Distance-estimation colour methods.
- Multi-colour and texture options.

The icon is rendered by fracst itself:
`cargo run --release -p fracst-core --example icon` regenerates `assets/icon.*`.

The screenshots are taken by the app itself. With `FRACST_SCREENSHOT=out.png`
it saves its own window once the render settles, then exits.
`FRACST_SCREENSHOT_RUN` runs palette entries first, by key (for example
`panel`, `make-video` or `focus:statistics`, separated by `;`), and
`FRACST_SCREENSHOT_SEARCH` opens the palette with a query. Point `APPDATA`
(Windows) or `XDG_CONFIG_HOME` (Linux) at an empty folder so saved settings,
recent files and remote keys stay out of the picture.

## Project layout

```
crates/fracst-core   engine: fixed.rs (bignum), floatexp.rs + num.rs (extended range),
                     view.rs, reference.rs, bla.rs, perturb.rs, render.rs, newton.rs,
                     colour.rs, kfr.rs, image.rs; examples bench, deepbench and icon
crates/fracst-cli    command-line renderer, and fracst-cli video
crates/fracst        egui explorer: app.rs (canvas, toolbar, settings card),
                     app/commands.rs (every action, for the toolbar, keys and palette),
                     search.rs (the Ctrl+K palette), glide.rs (zoom animation),
                     theme.rs, window.rs (frameless window), video.rs (Make video),
                     worker.rs (render thread), prefs.rs, remote_config.rs, update.rs
crates/fracst-cuda   CUDA pixel engine: kernel/fracst.cuh (the pixel loop, also
                     compiled for the host in tests), NVRTC compilation, check.rs
crates/fracst-remote remote protocol, lossless frame compression, client
crates/fracst-server WebSocket render server (CUDA when available)
crates/fracst-video  zoom videos: projects, keyframes, compositing, timeline, ffmpeg
deploy/fal           container image for fracst-server, and the fal.ai app
deploy/modal         the Modal app
gateway              Cloudflare Worker: fracst keys, usage limits, relay to the GPU
packaging            Windows installer (Inno Setup), Linux desktop entry and install.sh
docs                 performance log, remote rendering, zoom videos
```

## Roadmap

- A rescaled-`f64` deep phase. Extended-range steps are still about 6× slower
  than `f64` steps, and they dominate inside deep minibrots.
- Benchmarks of the CUDA engine on H100 and B200, and kernel tuning
  (occupancy, register use) against them.
- A lossy, server-coloured streaming mode for real-time navigation over slow links.
- The CUDA engine in the explorer itself, for local NVIDIA GPUs.
- More formulas: Burning Ship, higher powers, hybrids.
- Distance-estimation colouring and raw (EXR) output.

## License

Licensed under either of [Apache License 2.0](LICENSE-APACHE) or [MIT](LICENSE-MIT), at your option.

The bundled fonts, [Inter](assets/fonts/Inter-LICENSE.txt) and
[JetBrains Mono](assets/fonts/JetBrainsMono-LICENSE.txt), are under the SIL Open
Font License 1.1.
