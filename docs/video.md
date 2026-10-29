# Zoom videos

A zoom video in fracst starts at the whole set and ends at a location. It is
made in two stages. **Keyframes** are rendered once, which is slow. They are
then **composited and encoded** into the video, which is fast and can be
repeated with other colours, timing or music. Both the app (**Make video…**)
and `fracst-cli video` do this, with the same code (`crates/fracst-video`).

## Keyframes

Keyframe `k` is the location at zoom `Z / 2^k` (with the default factor of
2). Keyframes run from the location itself (`k = 0`) out past the start zoom
(1, where the image height spans 4), so a location at zoom 1e100 has about
334 of them. They are rendered deepest first. Each one:

- starts from the location's iteration limit, and automatic iterations raise
  it where the view needs more. The limit of the keyframe before is not
  carried outwards. Near a minibrot it can be millions, and further out
  interior pixels that aren't detected as interior would iterate to it.
- gets its own reference orbit. Going outwards, a reused one soon stops
  suiting the view: BLA skips less and interior goes undetected. In one test
  reuse made keyframes up to 60× slower. With the reference snapped to a
  minibrot nucleus, a fresh one costs milliseconds even at 1e397.
- is saved to `keyframes/kf_NNNNN.fkf` as soon as it is done. The file holds
  raw iteration data (smooth iteration counts), compressed losslessly with
  the remote protocol's codec, which makes it about 2–8 MB at 1080p. It is
  written to a temporary file and renamed, so a crash leaves no half-written
  keyframe behind.

Rendering resumes from the first missing keyframe, so a job can stop for any
reason and continue later, here or on another engine.

**Estimate** (`--estimate` in the CLI) renders the next keyframe and
multiplies its time by the number left. The deepest keyframes are usually
the slowest, so this is an upper bound. Views that cross the edge of the
main cardioid, around zoom 10 to 1000, can be slow too.

### On a remote GPU

Keyframes can render on a `fracst-server` (directly, or through Modal, fal
or the gateway), with the same URL and key as **Remote GPU**. If the server
announces the `queue` feature, two requests are kept in flight, so the GPU
renders the next keyframe while the last one downloads. Remote keyframes
are bit-identical to local ones.

If the connection drops, the job reconnects once and carries on. This also
covers the gateway's limit on session length. If reconnecting fails too,
the job **pauses** and shows the reason: for example, a gateway refusing a
key that has used its monthly minutes. Its keyframes are kept, and it can
resume later or finish on this computer. In the CLI, a paused job exits
with code 2 and prints the command that resumes it.

The gateway counts usage as time connected. The estimate after the first
remote keyframe is in those terms. In the app, the video job has its own
connection beside the explorer's, and a gateway key allows two sessions at
once by default.

## Frames

Every keyframe has the same centre, so a frame at zoom `z` is built from the
keyframes around `z`. Their zooms lie between `z / 4` and `4z` (`factor²`
either side). They are stacked from the outside in:

- the shallowest covers the whole frame.
- each deeper one covers a smaller centred part, in more detail, and fades
  out over a margin at its edges (4% of its height).
- the deepest fade in as they come into range.

Layers only ever enter or leave while they are fully covered, or fully
faded out. As a result nothing jumps when the zoom passes a keyframe. The
test video's frame-to-frame differences have no spikes.

Each layer is coloured at its own resolution, then sampled through a
mipmap, so that shrunken layers don't alias. Slope shading is applied after
sampling, scaled to output pixels, so layers of different scales shade
alike. Shading each layer at its own scale showed as visible rectangles.

### Quality

| Preset | Keyframe size | Samples per pixel | |
|---|---|---|---|
| Draft | 1× the video | 1 | Edges are magnified up to 2× just before a keyframe |
| Good | 1.5× | 2×2 | Up to 1.33× |
| Best | 2× | 3×3 | Never magnified |

The keyframe size and zoom factor are fixed once a keyframe is rendered.
Start a new project to change them.

## Timing

The zoom moves at a constant speed in zoom doublings per second. It speeds up
over **ease in** seconds and slows down to a stop at the location over **ease
out** seconds, with smooth (cosine) ramps. The video can also hold still at
the start and end. The length is set as one of:

- **per doubling**: seconds per zoom doubling at full speed. The total
  follows from the depth.
- **total**: the whole video, holds included.
- **fit audio**: as long as the audio track.

## Colours

The project starts with the location's colouring (palette, `IterDiv`,
offset, transfer function, slopes). On top of that:

- **Palette cycling** moves the offset by N palette entries per second.
- **Colour keys** set the colours at moments of the video: offset,
  `IterDiv`, and slope power, ratio and angle. Each field is eased
  separately between the keys that set it. `IterDiv` changes by ratio, so it
  moves evenly across orders of magnitude. In the app, **Add key from current
  colours** takes the explorer's colours at the preview's time.

Opening a project in the app moves the explorer to its location, so its
colours can be tuned there.

## Encoding

Frames are piped as raw RGB into ffmpeg, which writes BT.709 YUV with matching
tags:

| Codec | Encoder | Default quality |
|---|---|---|
| H.264 (`.mp4`) | libx264, preset slow, yuv420p | CRF 18 |
| H.265 (`.mp4`) | libx265, preset slow, tagged `hvc1` for Apple players | CRF 20 |
| AV1 (`.mp4`) | libsvtav1 if present, else libaom-av1 | CRF 28 |
| ProRes (`.mov`) | prores_ks, profile HQ, 4:2:2 10-bit | — |

Audio is encoded as AAC at 256 kb/s (PCM for ProRes). Audio shorter than the
video is padded with silence. Audio longer than the video fades out over the
last 2 seconds.

Without ffmpeg (or with `--frames`), frames are written as PNGs to
`frames/` in the project folder.

### Getting ffmpeg

fracst looks for ffmpeg in this order:

1. `FRACST_FFMPEG`.
2. One chosen with **Locate ffmpeg…** in the app, which fracst remembers.
3. One it downloaded earlier.
4. One on the `PATH`.
5. Where package managers put it: winget (including packages whose command
   links are missing), Scoop, Chocolatey and `C:fmpegbin` on Windows;
   Homebrew, MacPorts, `/usr/local/bin`, `/usr/bin`, Snap and `~/.local/bin`
   elsewhere. Apps opened from the macOS Finder don't get Homebrew's folder
   on their `PATH`, and winget can fail to create its links, so the `PATH`
   alone misses these.

If there is none, the app offers to download one or to locate one, and the
CLI asks (or downloads without asking with `--download-ffmpeg`). The builds
are pinned to ffmpeg 9.0.2 and checked against these SHA-256 checksums before
use:

| Platform | Source | Size |
|---|---|---|
| Windows x86-64 | gyan.dev essentials build (GitHub release) | 115 MB |
| macOS Apple Silicon, macOS Intel | ffmpeg.martin-riedl.de | 28 / 34 MB |
| Linux x86-64, Linux arm64 | ffmpeg.martin-riedl.de | 33 / 29 MB |

The executable goes into fracst's data folder: `%APPDATA%\fracst\ffmpeg` on
Windows, `~/Library/Application Support/fracst/ffmpeg` on macOS, and
`~/.local/share/fracst/ffmpeg` on Linux. Nothing else on the system changes,
and no administrator rights are needed. To update the pins, change
`DOWNLOADS` in `crates/fracst-video/src/ffmpeg.rs`.

## The project folder

```
location.kfr          the location (centre, zoom, colouring); never changes
video.toml            the settings below
keyframes/            kf_00000.fkf … (kf_00000 is the location itself)
video.mp4             the default output
frames/               PNG frames, when written
```

`video.toml` can be edited by hand. Only `[keyframes]` decides what the
keyframes look like; everything else applies when encoding.

```toml
width = 1920            # the video, in pixels (even)
height = 1080
fps = 60.0
quality = "good"        # draft, good, best

[keyframes]             # fixed once a keyframe is rendered
width = 2880
height = 1620
factor = 2.0            # zoom factor between keyframes
start_zoom = 1.0        # the video starts here
[keyframes.settings]    # rendering settings, as in the protocol
auto_iterations = true
reuse_reference = false
# …

[timing]
length = { per_doubling = 1.0 }   # or { seconds = 90.0 }, or "fit_audio"
ease_in = 2.0
ease_out = 4.0
hold_start = 0.0
hold_end = 2.0

[colour]
cycle = 0.0             # palette entries per second
[[colour.keys]]
time = 0.0
iter_div = 0.02
[[colour.keys]]
time = 60.0
iter_div = 0.5
slope_angle = 225.0

[encode]
output = "video.mp4"    # relative to the project folder, or absolute
codec = "h264"          # h264, h265, av1, prores
# crf = 18
# audio = "C:/music/song.flac"
```

## Speed

Measured on an 8-core Ryzen 7 8845HS, at a location at zoom 1e15 and one at
1e8, both near the seahorse valley:

- Keyframes at 960×540: 52 keyframes in under a minute.
- Keyframes at 2880×1620 (1080p Good): 29 keyframes in about 2.7 minutes.
  Most of the time went on the views near the main cardioid.
- Encoding 1080p Good: about 6 frames per second with fixed colours, and
  about 4 with animated colours, because every layer is recoloured each
  frame. 640×360 encodes at 35–50 frames per second.

## How it's tested

`cargo test -p fracst-video` covers the following:

- The `video.toml` round trip.
- The keyframe plan, and that it reaches past the start.
- Keyframe files.
- The speed curve: its length, monotonic and continuous zoom, and one
  doubling per second at full speed.
- Colour keys and cycling.
- Mipmaps, layer stacking, and equal slope shading at different scales.
- Reading durations from ffmpeg, and that every download is pinned.

`fracst-server`'s end-to-end tests cover queued requests.

These were run end to end by hand:

- Rendering, resuming and encoding locally.
- Rendering on a local `fracst-server`, killing it mid-job (the job paused),
  and resuming.
- H.264 and H.265, audio fitting, colour keys and PNG frames.
- The real ffmpeg download on Windows, into an empty data folder.
- Checking for spikes in frame-to-frame differences, to confirm the
  compositing doesn't pop.
