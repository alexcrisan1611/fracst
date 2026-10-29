# Brief: fracst mobile, an Expo app that explores on a cloud GPU

You are working in the fracst repository (Rust, Cargo workspace), on Windows 11.
fracst is a deep-zoom Mandelbrot explorer. It already renders on a cloud GPU:
a CUDA engine runs inside `fracst-server` on Modal or fal.ai, and a
Cloudflare Worker (`gateway/`) lets people connect with their own revocable
fracst keys (`fst_…`), keeping the Modal or fal token secret.

Build a mobile app with Expo and TypeScript that explores the Mandelbrot set
through that gateway. The phone does no fractal maths beyond exact view
arithmetic. It sends views, receives finished images and shows them with
fluid gestures.

The work has three parts, in this order:

1. The server learns to send coloured, compressed images.
2. The gateway learns mobile- and web-friendly authentication.
3. The app.

Read `docs/remote.md` first. It describes the remote architecture, the
protocol, the gateway and how everything is tested.

---

## Ground rules

- **Plan, then build in phases.** Plan before coding. After each phase,
  verify it (tests, type checks, a real run) before moving on. At the end,
  report what you verified and what you could not (for example iOS without a
  Mac, or the real Modal or fal deployment). Don't claim anything you didn't run.
- **Use current APIs.** Expo moves fast. Use `npx expo install <pkg>` for
  every Expo-ecosystem package, so versions match the SDK. Check
  `npx expo install --check` and `npx expo-doctor@latest`. Check current
  docs (docs.expo.dev, the package READMEs) rather than memory: APIs changed
  in 2025–2026.
- **No other projects.** fracst's own text (UI copy, docs, comments, store
  descriptions) must not mention or compare with other fractal software.
  Using libraries is fine.
- **Git.** Don't commit or push unless asked. Never add AI co-author
  trailers or "generated with" lines.
- **Screenshots.** Capture only the emulator or simulator
  (`adb exec-out screencap -p`, `xcrun simctl io booted screenshot`), or the
  browser page for web. Never capture the host desktop. The desktop app has
  its own hook (`FRACST_SCREENSHOT=path`).
- **Money.** Connecting to the real gateway, Modal or fal app starts a paid GPU.
  Develop against the local stack below, and ask before touching anything
  deployed.
- **Style.** Match the existing code. Rust: rustfmt with `rustfmt.toml`,
  clippy clean, comments that explain why. TypeScript: strict, small
  modules, no `any`.

---

## What exists (read these)

| Path | What |
|---|---|
| `crates/fracst-core` | Engine. `view.rs` (View: exact centre, zoom, maxiter), `fixed.rs` (fixed-point bignum, `parse`, `to_decimal`), `floatexp.rs` (extended-range float and its `Display`), `colour.rs` (Colouring, Palette presets, Transfer, Slopes; `Colouring::apply` → RGBA), `kfr.rs` (location files), `image.rs` (downscale, PNG) |
| `crates/fracst-remote` | Protocol (`protocol.rs`), lossless frame codec (`codec.rs`), blocking Rust client (`client.rs`): copy its skip and cancel logic |
| `crates/fracst-server` | WebSocket server (`lib.rs`) and its end-to-end tests (`tests/end_to_end.rs`) |
| `crates/fracst` | Desktop app (egui). `worker.rs` shows the render, preview and remote logic to mirror. `app.rs` shows auto-iteration handling (start the next frame from `stats.suggested_maxiter`) |
| `gateway/` | Cloudflare Worker: `src/index.ts` (routes, key checks, admin), `src/session.ts` (per-connection Durable Object relaying to the GPU), `src/registry.ts` (keys, usage, limits), `scripts/keys.mjs`, `test/smoke.mjs` |
| `deploy/modal/`, `deploy/fal/` | The Modal and fal apps, and the Dockerfile for the GPU server |

**Protocol today.** JSON text messages over a WebSocket, tagged by `"type"`
in snake_case.

- **Client → server:**
  - `render {id, view, settings, queue?}`. A newer render cancels the one in
    flight, unless it is sent with `"queue": true` to a server that lists the
    `queue` feature: then it waits its turn and frames come back in order.
  - `cancel {id}`.
- **Server → client:**
  - `hello {protocol, version, engine, features}`, where `features` (possibly
    absent) lists optional abilities such as `queue`. A gateway may send `status {message}`
    first, while a GPU starts.
  - `keepalive` every 30 s. Ignore it.
  - `progress {id, stage, fraction}`.
  - `frame {id, width, height, maxiter, de, blocks, bytes, stats, render_s, encode_s}`,
    followed by `blocks` binary messages.
  - `error {id | null, message}`.
- **Views are exact.** `view.re` and `view.im` are the centre's fixed-point
  limbs, as 16-hex-digit strings, least significant first. The last limb is
  the signed integer part, and the number is two's complement.
  `view.zoom` is `[mantissa, binary_exponent]` with the mantissa in [1, 2).
  The view also carries `maxiter` and `imag_down`.
- **Settings** mirror `fracst_core::Settings`. Missing fields take their
  defaults.
- **Abandoned frames.** Frames for abandoned request ids still arrive. A
  client must skip their `blocks` binary messages, as `client.rs` does.

**Local stack for development:**

```
cargo build --release -p fracst-server -p fracst-cli
target/release/fracst-server --listen 0.0.0.0:7878
cd gateway && cp .dev.vars.example .dev.vars    # ADMIN_TOKEN, UPSTREAM_URL=ws://127.0.0.1:7878
npm install && npx wrangler dev --ip 0.0.0.0     # gateway on :8787
FRACST_GATEWAY=http://127.0.0.1:8787 FRACST_ADMIN_TOKEN=dev-admin-token npm run keys -- create phone
```

Without a GPU the server renders on the CPU. The app can't tell the
difference.

- Android emulator: the host is `10.0.2.2`, so use `ws://10.0.2.2:8787/stream`.
- Physical devices: use the PC's LAN address.

---

## Part 1: image frames from the server (Rust)

Lossless frames are 6–9 MB at 1080p, and about 3× that at phone resolution.
Phones need the server to colour the frame and send a compressed image. Add
this as an optional extension, so existing clients keep working.

1. **Request.** `render` gains an optional `image` object:
   ```json
   "image": {
     "format": "jpeg",            // "jpeg" | "png"
     "quality": 88,               // JPEG only, 1-100
     "supersample": 1,            // 1-3: render ss times larger, downsample before encoding
     "colouring": {
       "palette": [[255,255,255],[64,0,128],[0,0,160]],   // key colours, RGB
       "iter_div": 0.1, "offset": 0, "transfer": "linear", // linear|sqrt|cbrt|fourth_root|log|log_log
       "smooth": true, "flat": false, "interior": [0,0,0],
       "slopes": {"power": 50, "ratio": 20, "angle_deg": 45}  // or null
     }
   }
   ```
   `settings.width` and `settings.height` are the output image size. With
   `supersample`, the server renders `width·ss × height·ss`, which counts
   against `--max-pixels`, and downsamples with `fracst_core::image::downscale`.
   Validate everything: an unknown transfer, an empty palette, quality or
   `supersample` out of range, or an oversized image gets an `error` for
   that id.
2. **Response.** The `frame` header gains `"image": "jpeg"` (absent for raw
   frames). The binary messages are the encoded image cut into ≤ 1 MiB
   pieces, concatenated in order. `de` is false. Stats are as before.
3. **Encoding.**
   - JPEG with the `jpeg-encoder` crate (pure Rust, SIMD). Use 4:4:4
     sampling, because fractals have fine colour detail that 4:2:0 smears.
   - PNG with the `png` crate (already a dependency): fast compression, no
     text chunks.
   - Encode on the worker thread, in parallel where the crates allow.
4. **Recolouring is instant.** Each connection keeps its last raw `Frame`
   and the view and settings that made it. If a request's view and settings
   equal the cached ones and only `image` differs, skip rendering: recolour
   and encode. This makes palette editing on the phone feel local.
5. **Capabilities.** `hello`'s `features` gains `"image"` and `"recolour"`
   (next to `"queue"`). The app refuses servers without `image`. Keep
   `PROTOCOL = 1`, since the change is additive.
6. **Subprotocols.** If a client offers the subprotocol `fracst.v1`, select
   it in the handshake response. Also accept the server token as a
   `bearer.<token>` subprotocol, for browsers, which can't set headers.
7. **Tests** (extend `crates/fracst-server/tests/end_to_end.rs`):
   - A PNG-mode frame decodes to exactly `Colouring::apply` of a local
     render of the same view.
   - JPEG-mode decodes to the right size, with PSNR ≥ 40 dB against the PNG.
   - Recolour requests skip rendering (assert via stats or timing that no
     second render happened, for example `stats.steps` unchanged and
     `render_s` near zero).
   - Bad image requests get errors.
   - The subprotocol is echoed.
   - `cargo test --workspace` and `cargo clippy --workspace --all-targets`
     are clean.

---

## Part 2: gateway changes (`gateway/`, TypeScript)

1. **Subprotocol authentication.** If there is no `Authorization` header,
   read `Sec-WebSocket-Protocol` and take the key from a `bearer.<key>`
   entry. Keys are base64url, which is valid in a subprotocol token. Answer
   with `Sec-WebSocket-Protocol: fracst.v1` from the session Durable
   Object's 101 response.
2. **In-band refusals.** For clients that offer `fracst.v1`, accept the
   WebSocket even when refusing, then send
   `{"type":"error","id":null,"message":…}` and close:
   - `4401` for a bad or revoked key,
   - `4429` for session or monthly limits,
   - `4503` when the gateway is not configured.

   Mobile WebSocket APIs don't expose HTTP status codes or headers on a
   failed upgrade, so this is the only way the app can say why. Keep HTTP
   status refusals for other clients: the desktop app and its tests rely on
   them.
3. **`GET /me`.** Authenticated with `Authorization: Bearer <key>` (a plain
   HTTP request, so browsers can send the header too). Returns
   `{name, monthly_minutes, used_minutes, open_sessions, max_sessions, max_session_minutes}`
   without opening a session, for the app's account screen.
4. **Handing out keys.** `scripts/keys.mjs create` also prints a deep link,
   `fracst://connect?gateway=<wss://…/stream>&key=<key>`, and that link as a
   terminal QR code (the `qrcode` package), so a phone can scan it.
5. **Tests.** Extend `test/smoke.mjs` to cover subprotocol auth, in-band
   refusal codes and `/me`. `npx tsc --noEmit` must stay clean.

---

## Part 3: the app (`mobile/`)

### Stack

Use the newest stable releases that work with each other: Expo SDK 58
(React Native 0.88, React 19.2, New Architecture only, Hermes) at the time of
writing. Verify with `npx expo install`, and prefer each library's current
major version.

- **Language and tooling:**
  - TypeScript strict, with `noUncheckedIndexedAccess`. React Native 0.87+
    uses the Strict TypeScript API, so no deep `react-native/Libraries`
    imports.
  - React Compiler enabled.
  - ESLint flat config (`eslint-config-expo`, with the React Compiler rules
    of `eslint-plugin-react-hooks`) and Prettier.
- **Navigation:**
  - Expo Router with typed routes, native stack, and native form sheets with
    detents (`presentation: "formSheet"`) for panels.
  - Native tabs only if they genuinely help; a single explorer screen with
    sheets is probably better.
- **Drawing:** `@shopify/react-native-skia` draws the fractal image with a
  transform matrix driven from the UI thread.
- **Gestures and animation:** `react-native-reanimated` 4 with
  `react-native-worklets`, and `react-native-gesture-handler`. Pinch, pan,
  double-tap and two-finger tap run at the display's refresh rate.
- **Native controls:** `@expo/ui` (SwiftUI and Jetpack Compose) for sliders,
  switches and pickers where it is stable; otherwise React Native
  components.
- **Platform looks:** `expo-glass-effect` (Liquid Glass) for the overlay on
  iOS, with `expo-blur` or a plain translucent fallback elsewhere.
  `expo-haptics` for zoom taps.
- **State and storage:**
  - Zustand for app state.
  - `react-native-mmkv` (current Nitro-based major) for settings, history
    and bookmarks.
  - `expo-secure-store` for the key (localStorage on web, with a note in
    the UI).
- **Validation:** Zod 4 schemas for every server message, mirroring
  `protocol.rs`.
- **Files and media:**
  - `expo-file-system` (the object API: `File`, `Directory`) for thumbnails
    and saved images.
  - `expo-media-library` and `expo-sharing` to save and share.
  - `expo-camera` (`CameraView` barcode scanning) for the QR code.
  - `expo-linking` for deep links (scheme `fracst`).
- **Lists:** `@shopify/flash-list` v2 or `@legendapp/list` for bookmarks
  and history.
- **Styling:** NativeWind v5 (Tailwind CSS v4) if it is stable on SDK 58;
  otherwise StyleSheet with a small token module. Dark, image-first theme.
- **Builds:**
  - `expo-dev-client` development builds (Skia and MMKV need them).
  - EAS Build and EAS Update configs (`eas.json` with development, preview
    and production profiles).
  - Bundle id `io.github.alexcrisan1611.fracst`, app name "fracst".
  - Icon and splash generated from `assets/icon.svg` (rendered by fracst
    itself).
- **Configuration:**
  - The default gateway comes from `EXPO_PUBLIC_FRACST_GATEWAY` at build
    time, set as an EAS environment variable.
  - Users can always change it.

### Structure

```
mobile/
  app/                     Expo Router: _layout.tsx, index.tsx (explorer), connect.tsx (key: paste or scan),
                           colour.tsx (form sheet), settings.tsx, locations.tsx (bookmarks and history), +not-found.tsx
  src/view/                exact view maths (BigInt): fixed.ts, floatexp.ts, view.ts, kfr.ts
  src/protocol/            schemas.ts (Zod), client.ts (FracstClient), images.ts
  src/explorer/            canvas (Skia), gestures, render scheduler, HUD
  src/colour/              palette presets (ported from colour.rs), colouring model
  src/state/               Zustand stores, persistence (MMKV, SecureStore)
  test/                    unit tests and vectors/
```

### Exact view maths (`src/view`)

Port the parts of `fixed.rs`, `floatexp.rs` and `view.rs` the app needs to
TypeScript with BigInt (Hermes supports it), bit for bit:

- `Fixed`: the limbs ↔ BigInt in two's complement, and the hex limbs of the
  protocol, `parse` (decimal strings, as `Fixed::parse`), `to_decimal`,
  `from_floatexp`, `add` and `sub`.
- `FloatExp`: mantissa and exponent, multiply, divide, `parse`, `log2`, and
  `Display` (for example `1.234567E456`).
- `View`:
  - `default`, `limbs_for`, `ensure_precision` and `pixel_spacing`;
  - `pixel_offset`, honouring `imag_down`;
  - `translate_pixels` and `zoom_at`.

These must match Rust exactly, or shared locations drift. Add
`crates/fracst-core/examples/view_vectors.rs`, which prints JSON test
vectors: parses, decimal strings, sequences of zooms and translations at
depths from 1 to 1e3000, and the limbs before and after each step. Commit
its output to `mobile/test/vectors/view.json`, and make the TypeScript unit
tests reproduce every vector exactly. Run the heavy BigInt work off the
gesture path, on the JS thread after a gesture ends.

### The client (`src/protocol/client.ts`)

- **Connecting.**
  - Native: `new WebSocket(url, ["fracst.v1"], { headers: { Authorization: "Bearer <key>" } })`.
  - Web: `new WebSocket(url, ["fracst.v1", "bearer.<key>"])`.
  - `binaryType = "arraybuffer"`.
- **States:** idle, connecting (with the gateway's `status` text), connected
  (engine, version, features), failed (with the in-band error and close
  code: `4401` routes to the key screen).
- **Rendering.** `render(view, settings, image)` returns a promise of
  `{ bytes, header }` and takes an `AbortSignal`.
  - Only the newest request matters: a new one supersedes the old, and
    abort sends `cancel`.
  - Skip the binary messages of abandoned frames exactly as `client.rs`
    does.
  - Surface progress messages.
  - Ignore `keepalive`.
  - Reassemble the image pieces.
- **Connection hygiene.** Every connected minute can bill a GPU.
  - Disconnect when the app goes to the background (`AppState`) and
    reconnect on return.
  - Disconnect after 5 idle minutes in the foreground (configurable).
  - Reconnect transparently on the next interaction.
  - Handle network changes.

### The explorer (`app/index.tsx`)

- **The image.** Full-screen, edge to edge, under the safe areas. The
  rendered image is drawn with Skia. While a gesture runs, the last image is
  transformed live (scale and translate, on the UI thread) so it follows
  the fingers, like the desktop app shows the previous frame stretched to
  the new view.
- **Gestures.**
  - When a gesture ends, compute the exact new view (`zoom_at` around the
    pinch focus, `translate_pixels` for the pan) and request it.
  - Double-tap zooms in ×4 (a setting) around the tapped point.
  - Two-finger tap zooms out.
  - Light haptics.
- **Requests.**
  - The size is the screen's physical pixels × a resolution setting
    (default 0.75), as JPEG (default quality 85), with auto iterations on.
  - If the last full frame's round trip took over 300 ms, ask for a
    quarter-size preview first, then the full frame, as `worker.rs` does.
  - After each full frame, carry `stats.suggested_maxiter` into the view.
  - While a newer view is pending, keep showing the transformed old image.
    Never show a frame for a view that has since changed.
- **HUD** (glass overlay, collapsible): zoom (FloatExp `Display`),
  iterations, engine (for example "NVIDIA H100"), round trip, a thin
  progress bar during renders, and the cold-start status ("Waiting for a
  GPU on fal.ai… 40 s").
- **Colour sheet.** Palette presets with swatches (port
  `Palette::presets`), a random palette (port the seeded generator), key
  colours, `iter_div` (logarithmic), `offset`, transfer, smooth, flat,
  interior colour and slopes. Changes re-request with the same view, so the
  server only recolours. Debounce slider drags to about 10 requests a
  second.
- **Locations.** History (automatic) and bookmarks (named), each with a
  thumbnail, stored in MMKV with the exact view and colouring.
  - Share as a link
    (`fracst://view?re=…&im=…&zoom=…&iterations=…`, decimals from
    `to_decimal` with `20 + zoom digits` digits, as `View::digits`) or as
    `.kfr` text (see `kfr.rs`).
  - Opening such a link restores the view exactly.
- **Save image.** Re-render at screen size × 2 with `supersample` 2 as PNG,
  then save to Photos or share. Show progress, and the location in the
  share text.
- **Connect screen.** Paste a key, scan the QR code from `keys.mjs`, or
  open the `fracst://connect` link. Show usage from `GET /me` ("42 of 600
  GPU minutes this month").
- **Settings:** gateway address, key (masked; replace or forget),
  resolution, JPEG quality, zoom factor, auto iterations, interior
  detection, idle disconnect, and under "Advanced": BLA, reference
  snapping, max iterations.
- **Polish:**
  - Portrait and landscape, tablets.
  - Dynamic Type and accessibility labels.
  - Reduced-motion support.
  - Clear empty and error states (no key, refused key, GPU starting,
    offline).

### Tests and verification

- **Unit tests** (`jest-expo` and React Native Testing Library):
  - View maths against the Rust vectors.
  - The protocol schemas on fixtures captured from a real `fracst-server`.
  - The client state machine with a fake WebSocket: status before hello,
    supersede and cancel, skipping abandoned blocks, keepalive, in-band
    errors, reconnecting.
  - Colouring model to JSON.
  - The render scheduler: preview logic, never showing stale frames.
- **Integration** (Node, against the local stack): a script uses
  `src/protocol/client.ts` (with the `ws` package standing in for React
  Native's WebSocket) to connect through `wrangler dev`, render image-mode
  frames and check they decode at the requested size.
- **On a device:**
  - A development build on the Android emulator (Android Studio) against
    the local stack. Exercise every screen and take emulator screenshots.
  - `npx expo start --web` in a browser, which exercises subprotocol auth.
  - iOS needs a Mac or EAS Build: prepare the profiles, and say plainly if
    it wasn't run.
- **End to end:** a Maestro flow for the main path (connect with a key, see
  an image, double-tap zoom, open the colour sheet, change preset).
- **Gates:** `npx tsc --noEmit`, ESLint, Jest and `npx expo-doctor` all
  clean, plus the Rust and gateway checks from Parts 1 and 2.

### Docs

- `mobile/README.md`: run, build and test.
- A "Mobile app" section in `docs/remote.md`: how keys reach phones (QR or
  link), costs, connection hygiene.
- A short mention in the top-level README.

---

## Definition of done

The image extension and the gateway changes are in, tested and documented.
The app runs on the Android emulator and on web against the local stack,
covering every feature above with screenshots. All checks are green. The
final report lists what was verified, what wasn't (iOS on a device, the real
fal and Cloudflare deployments) and any open decisions.
