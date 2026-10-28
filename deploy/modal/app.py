"""fracst on Modal: a cloud GPU render server for the fracst app.

The image is the one in deploy/fal/Dockerfile, without fal. Modal starts
fracst-server on a GPU and exposes its WebSocket directly:

    wss://<workspace>--fracst.modal.run    (Modal-Key / Modal-Secret headers)

Requests need a Modal proxy auth token, checked by Modal before anything
starts, so nobody else can wake a GPU. Deploy from the repository root
(see docs/remote.md):

    modal deploy deploy/modal/app.py
    modal run deploy/modal/app.py::selftest     # CPU and GPU must match

Billing runs while a container is up: while a client is connected, plus
`scaledown_window` seconds after. fracst-server closes connections that
send nothing for IDLE_TIMEOUT seconds, so a forgotten window doesn't keep
a GPU running. At most `max_containers` containers ever start.
"""

import json
import os
import subprocess
from pathlib import Path

import modal

SERVER = "/usr/local/bin/fracst-server"
PORT = 7878
# Seconds without requests before fracst-server drops a connection.
IDLE_TIMEOUT = int(os.environ.get("FRACST_IDLE_TIMEOUT", "900"))
# H100s are the cheapest GPUs with fast FP64, which the pixel loop needs;
# Modal falls back along the list when none is free. Override with
# FRACST_MODAL_GPU=B200 (or a comma-separated list) at deploy time.
GPU = os.environ.get("FRACST_MODAL_GPU", "H100,H200,B200").split(",")

# Paths only matter where the image is built from; the module is imported
# again inside the container, where the repository isn't.
REPO = Path(__file__).resolve().parents[2] if modal.is_local() else Path("/")
image = modal.Image.from_dockerfile(
    str(REPO / "deploy/fal/Dockerfile"),
    context_dir=REPO,
    build_args={"PLATFORM_PIP": ""},
)

app = modal.App("fracst")


@app.function(
    image=image,
    gpu=GPU,
    min_containers=0,
    max_containers=1,
    scaledown_window=300,
    # A connection lasts as long as the client stays connected.
    timeout=4 * 3600,
)
@modal.concurrent(max_inputs=8)
@modal.web_server(PORT, startup_timeout=120, label="fracst", requires_proxy_auth=True)
def serve():
    """The fracst remote protocol, straight from fracst-server."""
    subprocess.Popen([SERVER, "--listen", f"0.0.0.0:{PORT}", "--idle-timeout", str(IDLE_TIMEOUT)])


def run_report(flag: str, timeout: int) -> dict:
    done = subprocess.run([SERVER, flag, "--json"], capture_output=True, text=True, timeout=timeout)
    try:
        report = json.loads(done.stdout.strip().splitlines()[-1])
    except (ValueError, IndexError):
        report = {"stdout": done.stdout, "stderr": done.stderr}
    result = {"ok": done.returncode == 0, "report": report}
    print(json.dumps(result, indent=2))
    return result


@app.function(image=image, gpu=GPU, timeout=1800)
def selftest() -> dict:
    """Render test views on the CPU and the GPU; they must match exactly."""
    return run_report("--selftest", timeout=1800)


@app.function(image=image, gpu=GPU, timeout=3600)
def bench() -> dict:
    """Time the CPU and the GPU on the benchmark locations at 1920x1080."""
    return run_report("--bench", timeout=3600)
