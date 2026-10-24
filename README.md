<img src="assets/icon.png" width="96" height="96" alt="fracst icon">

# fracst

A fast deep-zoom Mandelbrot set explorer written in modern Rust: an
alternative to [Fraktaler 3](https://fraktaler.mathr.co.uk/) focused on the
Mandelbrot set, with automatic iteration limits and minibrot-aware references.

- Zoom to 10^1000 and beyond, interactively.
- Opens and saves `.kfr` location files (Fraktaler 3 can import them), and
  embeds the location in saved PNGs so images can be reopened.
- Palette colouring: 1024-entry palettes interpolated from key colours,
  `IterDiv`, `ColorOffset`, transfer functions, and neighbour-difference slope shading.
- Newton's method minibrot finder ("jump to the nearest minibrot").
- CLI for batch renders, supersampling and zoom-out sequences for videos.

![fracst exploring a spiral at zoom 2.2e238](assets/screenshot.png)

## Download

Prebuilt 64-bit packages for **Windows**, **macOS** (universal: Apple Silicon and Intel)
and **Linux** are on the [Releases page](https://github.com/alexcrisan1611/fracst/releases/latest).
Each contains the explorer (`fracst`) and the command-line renderer (`fracst-cli`).
The builds are not code-signed. On Windows choose **More info → Run anyway** if
SmartScreen asks. On macOS right-click the app and choose **Open** the first time.

To build from source instead, install [Rust](https://rustup.rs) and run
`cargo run --release -p fracst`.

## How it compares with Fraktaler 3

Both render with perturbation, *bilinear approximation* (BLA), which skips
iterations anywhere along the orbit in steps of up to 2^k, and *rebasing*,
which needs only one reference orbit and avoids glitches by construction.
Where they differ:

| | Fraktaler 3 | fracst |
|---|---|---|
| Reference orbit | Image centre. A known period can be locked by hand | **Snaps to a nearby minibrot's nucleus** (Newton's method) and stops after one period |
| Iteration limit | Set by hand | **Automatic**: predicted from nearby minibrots, corrected per frame (see below) |
| Interior detection | Derivative with respect to z₁ | **Cycle multiplier at period ends** with a periodic reference, z₁ derivative otherwise |
| Past 1e300 | One extended number type for the whole frame | Extended range (shared-exponent complex) **only while a pixel's offset is tiny**, then plain `f64` |
| BLA tolerance | 2^-24 | 2^-40 by default (see Accuracy) |
| Reference arithmetic | MPFR | Custom fixed-point bignum (squarings only, truncated products) |
| Formulas | Mandelbrot, Burning Ship, hybrids and more | Mandelbrot z² + c only |
| GPU | OpenCL | CPU only, for now |

BLA and rebasing were introduced by Zhuoran on fractalforums. The theory is
written up by Claude Heiland-Allen in [deep zoom theory and practice](https://mathr.co.uk/web/deep-zoom.html)
and used in his [Fraktaler 3](https://fraktaler.mathr.co.uk/). fracst
implements these published algorithms independently and contains no code from
Fraktaler 3.

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
-p fracst-core --example bench -- --baseline`. Times are the first render
including the reference orbit, and vary by about ±30% between runs on this
laptop (boost clocks and thermals).

| Location | Zoom | Avg iterations | fracst | Plain perturbation | Speedup |
|---|---|---:|---:|---:|---:|
| Overview | 1 | 11 | 0.02 s | 0.05 s | 2× |
| Seahorse minibrot | 3e8 | 2,929 | 2.6 s | 3.4 s | ~1× |
| Spiral | 2e238 | 2,186 | 0.13 s | 2.2 s | 17× |
| Spiral | 8e397 | 4,024 | 0.25 s | 19.4 s | 76× |
| Spiral | 1e998 | 11,769 | 0.35 s | 57.8 s | 165× |
| Inside a minibrot (21% interior, 1M max iterations) | 2e400 | 136,874 | 6.0 s | not run | — |

"Plain perturbation" is the same engine with BLA and interior detection
turned off (one reference with rebasing). It shows what the iteration
skipping buys, and is not a comparison with another program. At shallow
zooms such as the seahorse row, BLA cannot skip anything and fracst runs at
plain perturbation speed.

For a head-to-head test, `--export` writes every benchmark location to
`bench-locations/*.kfr`. Fraktaler 3 imports `.kfr` locations (but not their
palettes), so you can render the same views there and compare.

## Accuracy

`cargo test --release` renders test views at zooms from 1e6 to about 1e400:
shallow and deep minibrots, spirals, and a reference sitting exactly on a nucleus.
It compares sampled pixels against brute-force fixed-point iteration of
the same points at higher precision. The renders must agree on the escape
iteration, and interior detection may not flag any pixel that escapes. All
of these run with and without BLA.

The BLA tolerance defaults to ε = 2^-40. Fraktaler 3 hard-codes 2^-24, but
in the tests it misplaced more than 1% of pixels near a minibrot. The
tighter tolerance costs only a few percent more steps. You can change it in
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
| Ctrl+O / Ctrl+S / Ctrl+E | Open location, save `.kfr`, export PNG |
| Drop a file | Open a `.kfr` or PNG with an embedded location |

The side panel covers:

- **Location**: arbitrary-precision coordinates, plus copy and paste as `.kfr` text.
- **Find minibrot**: Newton's method.
- **Auto iterations**.
- **Colouring**: palette presets, a random palette, editable key colours,
  `IterDiv`, offset, transfer function, slopes and interior colour.
- **Rendering**: resolution scale, BLA, interior detection, reference snapping,
  bailout and reference reuse.
- **Export**: any size with supersampling.
- **Statistics**: per-stage timings, BLA skip factor and iteration range.

While a frame renders, the previous one is shown stretched to the new view.
Slow frames get a quarter-resolution preview first.

### Command line (`fracst-cli`)

```
# Render a location (or a PNG with an embedded location) at 1080p with 2x2 supersampling
fracst-cli location.kfr -W 1920 -H 1080 --supersample 2 -o out.png

# Coordinates directly, any number of digits
fracst-cli --re -0.7436438870371587 --im 0.1318259042053120 --zoom 1e12 -i 50000 -o seahorse.png

# Jump to the nearest minibrot and frame it at 1/4 of the image height
fracst-cli location.kfr --find-minibrot --mini-zoom 0.25 --save-kfr -o mini.png

# Zoom-out sequence for videos (reuses the reference orbit throughout)
fracst-cli deep.kfr --zoom-out 500 --zoom-out-factor 2 -o frames/zoom.png
```

`fracst-cli --help` lists every option.

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

## Project layout

```
crates/fracst-core   engine: fixed.rs (bignum), floatexp.rs + num.rs (extended range),
                     reference.rs, bla.rs, perturb.rs, render.rs, newton.rs,
                     colour.rs, kfr.rs, image.rs
crates/fracst-cli    command-line renderer
crates/fracst        egui explorer (render thread in worker.rs)
```

## Roadmap

- A rescaled-`f64` deep phase. Extended-range steps are still about 6× slower
  than `f64` steps, and they dominate inside deep minibrots.
- GPU rendering (wgpu compute).
- More formulas: Burning Ship, higher powers, hybrids.
- Distance-estimation colouring and raw (EXR) output.

## License

Licensed under either of [Apache License 2.0](LICENSE-APACHE) or [MIT](LICENSE-MIT), at your option.
