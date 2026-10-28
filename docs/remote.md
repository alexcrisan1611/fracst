# Remote GPU rendering

fracst can render on another machine, typically a cloud GPU, while you
explore in the app on your own computer. The app sends each view to a
`fracst-server` and gets back the finished frame. Colouring, recolouring
and everything else in the interface stay local, so they are instant
however far away the server is.

```
fracst app / fracst-cli  ──WebSocket──>  fracst-server  ──>  CUDA engine (GPU)
  (your computer)          views ↑        reference orbit,     pixel loop
                           frames ↓       BLA tables (CPU)
```

## What runs where

- **The server's CPU** computes the reference orbit and the BLA tables, as
  locally. They are serial or cheap, and a GPU would not speed them up.
- **The GPU** iterates every pixel of a pass in one launch (`fracst-cuda`).
  The kernel is a line-by-line port of the CPU pixel loop: the same
  operations in the same order, with multiply-adds fused exactly where the
  CPU fuses them. Its frames are bit-identical to the CPU's.
- **Without a GPU**, or with `--cpu`, the server renders on its CPU, which
  is useful for a big machine shared by several people.

The pixel loop needs fast 64-bit floating point. Data-centre GPUs with full
FP64 rates (A100, H100, H200, B200) are the ones that pay. Blackwell Ultra parts
(B300, GB300) and gaming cards run FP64 at a small fraction of their 32-bit
speed. The kernels compile for them, but don't expect miracles.

## On Modal

`deploy/modal/app.py` runs the same image (`deploy/fal/Dockerfile`, built
without fal) on Modal, which exposes `fracst-server`'s WebSocket directly:

```
wss://<workspace>--fracst.modal.run
```

Modal checks a proxy auth token before anything starts, so nobody else can
wake a GPU. Create one under Settings → Proxy Auth Tokens; it has an id
(`wk-…`) and a secret (`ws-…`). In the app's **Key** field, give it as
`wk-…:ws-…`, or put it on the gateway (below) and hand out fracst keys.

Deploy with the **Deploy to Modal** workflow (Actions tab, or push a
`modal-deploy-*` tag). It needs a Modal API token as the repository secrets
`MODAL_TOKEN_ID` and `MODAL_TOKEN_SECRET`, and runs the GPU self-test after
deploying. Or from a machine with Python 3.11:

```
pip install modal
modal token new
modal deploy deploy/modal/app.py
modal run deploy/modal/app.py::selftest     # CPU and GPU must match
modal run deploy/modal/app.py::bench
```

The app asks for an A100 and falls back to an 80 GB A100 or an H100; set
`FRACST_MODAL_GPU` to choose. An A100 costs half as much as an H100 and is
about a third as fast at FP64. Modal bills the time a GPU is up, not the
work it does, and most of that time it waits for you, so the A100 costs
least. At most one GPU container runs, serving up to eight connections. It
stops a minute after the last one closes, and `fracst-server` drops
connections that send nothing for two minutes.
Modal limits WebSocket messages to 2 MiB; frames travel in compressed
blocks of at most 1 MiB of pixels, so they fit.

## On fal.ai

`deploy/fal/` holds a fal app: a container image with `fracst-server`
(`Dockerfile`) and a thin Python layer (`app.py`) that starts the server on
each GPU runner and passes WebSocket connections through to it.

### Deploy

You need a fal.ai account and Python 3.11 (the app is pickled at deploy
time, and the image's Python must match). From the repository root:

```
pip install fal
fal auth login                      # or set FAL_KEY
fal deploy deploy/fal/app.py::Fracst
```

The first deploy builds the image on fal's builders, which takes a few
minutes. It prints the app's address. The stream endpoint is:

```
wss://fal.run/<your-fal-user>/fracst/stream
```

The app asks for an H100 and falls back to an H200 or B200 when none is
free. To choose, set `FRACST_FAL_MACHINE` when deploying, for example
`FRACST_FAL_MACHINE=GPU-B200 fal deploy ...`.

To deploy without installing fal locally, add an ADMIN-scoped fal key as
the repository secret `FAL_KEY` and run the **Deploy to fal** workflow
from the Actions tab (it takes the machine types as an input), or push a
`fal-deploy-*` tag. The job summary shows the app's address.

### Connect

In the app, open **Remote GPU**, paste the stream address and your fal key,
and press **Connect**. Rendering continues locally until the GPU is ready.
A runner that was idle takes a minute or two to start. From then on, the
panel shows which GPU renders, and **Statistics** shows each frame's round
trip next to the server's own time. If the connection drops, the app goes
back to rendering locally and says so.

After a minute without renders, the app disconnects so the GPU can shut
down, and the panel says it is paused. The next render reconnects, in about
a second while the GPU is still up; until then it renders locally.
Recolouring never needs the GPU.

The app can also connect on start-up:
`FRACST_REMOTE_URL=wss://fal.run/... FAL_KEY=... fracst`.

From the command line:

```
fracst-cli --remote wss://fal.run/<your-fal-user>/fracst/stream deep.kfr -W 3840 -H 2160 -o deep.png
```

It reads the key from `--key`, `$FRACST_REMOTE_KEY` or `$FAL_KEY`, and
reports the round trip, the server's time and the bytes received.

### Check the GPU

Two HTTP endpoints run the server's built-in checks on the GPU:

```
curl -X POST -H "Authorization: Key $FAL_KEY" https://fal.run/<your-fal-user>/fracst/selftest
curl -X POST -H "Authorization: Key $FAL_KEY" https://fal.run/<your-fal-user>/fracst/bench
```

- **`/selftest`** renders twelve small views covering every path of the
  pixel loop on the CPU and on the GPU, and reports whether each pair of
  frames is identical. It counts passes the GPU actually rendered, so a
  silent fallback to the CPU fails.
- **`/bench`** times the six benchmark locations at 1920×1080 on the CPU
  and the GPU.

### Costs

fal bills a GPU runner by the second while it is up: while a client is
connected, plus five minutes (`keep_alive`) after the last one leaves.
`fracst-server` closes connections that send no requests for 15 minutes
(`FRACST_IDLE_TIMEOUT` in `app.py`), so a forgotten window does not keep a
GPU running. The app never starts more than one runner
(`max_concurrency = 1`), and up to eight clients share it.

## Sharing it: the Cloudflare gateway

Pasting your Modal or fal token into the app works for you alone. To let
other people render on your GPU, put `gateway/`, a Cloudflare Worker, in
front of it:

```
fracst app ──wss, fracst key──> Cloudflare gateway ──wss, Modal/fal token──> fracst-server
```

The gateway accepts the app's connection with a **fracst key** (`fst_…`),
opens its own connection to the GPU server with your token, and relays the
fracst protocol both ways unchanged. Frames stay exact, and the token never
leaves Cloudflare.

- **Keys are per person.** You create one for each person and can revoke it
  at any time. A revoked key's open sessions end within a minute. Only
  SHA-256 hashes of the keys are stored.
- **Limits.** Each key can have a monthly limit of GPU minutes (connected
  time, which is roughly what Modal or fal bills). Without a limit, usage is still
  counted. A key may have two sessions open at once (`MAX_SESSIONS_PER_KEY`),
  and a session lasts at most two hours (`MAX_SESSION_MINUTES`). The app
  can always reconnect.
- **Cold starts.** While a GPU starts, the gateway keeps the app informed
  ("Waiting for a GPU… 40 s"), and the Remote GPU panel shows it.

It runs on the Workers Free plan. Each connection is a Durable Object, whose
CPU budget renews with every message, so long sessions are fine. A single
Durable Object holds the keys, usage and open sessions.

Why not one key built into the app? fracst is open source, so anything in
the binary is public, and anyone could spend your GPU budget with it.

### Set up the gateway

Deploy the GPU server first (above). Then, with a Cloudflare account:

```
cd gateway
npm install
npx wrangler login
#   in wrangler.jsonc, set "UPSTREAM_URL": "wss://<workspace>--fracst.modal.run"
npx wrangler secret put MODAL_KEY      # the proxy auth token's id, wk-…
npx wrangler secret put MODAL_SECRET   # and its secret, ws-…
#   (for fal instead: "FAL_APP": "<your-fal-user>/fracst" and FAL_KEY)
npx wrangler secret put ADMIN_TOKEN    # any long random string, for managing keys
npx wrangler deploy
```

`wrangler deploy` prints the gateway's address,
`https://fracst-gateway.<your-subdomain>.workers.dev`.

### Hand out keys

```
export FRACST_GATEWAY=https://fracst-gateway.<your-subdomain>.workers.dev
export FRACST_ADMIN_TOKEN=<the ADMIN_TOKEN>
npm run keys -- create alice --minutes 600   # a key with 10 GPU hours a month
npm run keys -- list                         # usage this month
npm run keys -- revoke alice
npm run keys -- sessions                     # who is connected now
npm run keys -- selftest                     # the GPU self-test on fal
```

Each key is shown once. In the app, the person enters
`wss://fracst-gateway.<your-subdomain>.workers.dev/stream` as the server and
their key, then connects. The app remembers both (in `remote.txt` in its
configuration folder), but never connects on its own, since connecting may
start a paid GPU.

To have release builds suggest your gateway, set the repository variable
`FRACST_DEFAULT_REMOTE_URL` (Settings → Secrets and variables → Actions →
Variables) to its `wss://…/stream` address. Users then only paste a key.

### Test it locally

`wrangler dev` runs the gateway, Durable Objects included, on your own
machine. Point it at a local fracst-server:

```
fracst-server --listen 127.0.0.1:7878
cd gateway
cp .dev.vars.example .dev.vars          # ADMIN_TOKEN, and UPSTREAM_URL=ws://127.0.0.1:7878
npm run dev                             # http://127.0.0.1:8787
FRACST_GATEWAY=http://127.0.0.1:8787 FRACST_ADMIN_TOKEN=dev-admin-token npm run smoke
```

The smoke test checks key creation and refusal, the session limit, status
messages before the server's hello, and that revoking a key ends its open
sessions. `npm run keys -- create me` then gives a key for
`fracst-cli --remote ws://127.0.0.1:8787/stream --key fst_…`.

## On your own GPU machine

The release archives for Linux and Windows include `fracst-server`. It
loads the NVIDIA driver and NVRTC (the CUDA runtime compiler) when it
starts, and compiles the kernels for whatever GPU it finds. NVRTC comes
with the CUDA toolkit, or `pip install nvidia-cuda-nvrtc-cu12` with its
`lib` directory on `LD_LIBRARY_PATH` (`PATH` on Windows).

```
fracst-server --listen 0.0.0.0:7878 --token SECRET
```

In the app, connect to `ws://host:7878` with `SECRET` as the key. The token
travels in plain text over `ws://`, so across the internet use an SSH
tunnel instead of opening the port:

```
ssh -L 7878:localhost:7878 gpu-box     # then connect to ws://localhost:7878
```

The image in `deploy/fal/Dockerfile` also works on any Docker host with the
NVIDIA container runtime:

```
docker build -f deploy/fal/Dockerfile -t fracst-server .
docker run --gpus all -p 7878:7878 fracst-server fracst-server --listen 0.0.0.0:7878 --token SECRET
```

Other options:

- `--selftest` and `--bench` run the checks above locally.
- `--nvrtc-check sm_90,sm_100` compiles the kernels without a GPU, to check
  an installation.
- `--cpu` ignores the GPU.
- `--idle-timeout SECONDS` closes idle connections.
- `--max-pixels N` caps the image size.

## The protocol

JSON text messages over a WebSocket (`fracst-remote`):

1. The server says hello with its engine and version.
2. The client sends `render` requests, each with an id, the view and the
   settings. A newer request, or `cancel`, stops the one in flight.
3. While rendering, the server reports progress, then sends a `frame`
   header followed by the frame data in binary blocks.
4. An idle connection gets a `keepalive` every 30 seconds, so proxies keep
   it open. Keepalives don't count as activity, so the server's idle timeout
   still applies. A gateway may send `status` messages before the server's
   hello.

Views travel exactly: the centre's fixed-point limbs, and the zoom's
mantissa and exponent. A remote frame is therefore identical to a local
render of the same view, so saved PNGs and `.kfr` files are the same
wherever they were rendered.

Frames are lossless, so the app can recolour them and save them like
local ones. Iteration counts, fractional parts and distance estimates are
delta-coded against the left neighbour, split into byte planes and
compressed with zstd in independent 1 MiB blocks. Both ends work on the
blocks in parallel, and each fits in one WebSocket message.

Fractional parts carry nearly 32 bits of real detail per pixel, so a
detailed 1080p frame is about 6–9 MB, and a smooth one 2–3 MB. Over a
100 Mbit/s link that is up to about 0.7 s per full frame. Quarter-resolution
previews arrive 16 times faster. A lossy mode coloured on the server
(images or video) would allow real-time navigation over slower links.

## How it's tested without a GPU

- **The kernel source also compiles as plain C++** (the `host-kernel`
  feature of `fracst-cuda`). That build runs exactly the code the GPU runs,
  and `cargo test -p fracst-cuda --features host-kernel` checks that its
  frames match the CPU's bit for bit on the same views as `/selftest`. It
  runs without flushing subnormals, as on the GPU.
- **`tests/nvrtc.rs` compiles the CUDA kernels with NVRTC** for sm_80,
  sm_90, sm_100, sm_103 and sm_120 when `FRACST_NVRTC_TEST=1` is set and NVRTC is
  on the library path. NVRTC needs no GPU.
- **`fracst-server`'s end-to-end tests** run a real server and client over
  a local WebSocket. They check exact frames (with the host build of the
  kernel as the server's engine), tokens, cancellation, bad requests and
  idle timeouts.
- **The fal relay in `app.py`** was exercised under FastAPI and uvicorn
  with the real `fracst-server` behind it.
- **The gateway** was run under `wrangler dev` (the Workers runtime, with
  Durable Objects) between `fracst-cli` or the app and a real
  `fracst-server`. Frames came through bit-identical, and `npm run smoke`
  passes. A cold start was simulated by starting the server 12 seconds
  after the app connected.

Running on an actual GPU on fal is the one step that needs an account.
`/selftest` covers it.
