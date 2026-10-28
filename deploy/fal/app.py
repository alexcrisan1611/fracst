"""fracst on fal.ai: a cloud GPU render server for the fracst app.

The image (Dockerfile next to this file) contains fracst-server, which
renders frames with its CUDA engine. This app starts it on each runner and
passes WebSocket connections through to it, so the fracst app (or
`fracst-cli --remote`) can render on the GPU:

    wss://fal.run/<your-fal-user>/fracst/stream    (Authorization: Key <FAL_KEY>)

Deploy from the repository root (see docs/remote.md):

    fal deploy deploy/fal/app.py::Fracst

Billing runs while a runner is up: while a client is connected, plus
`keep_alive` seconds after. fracst-server closes connections that send
nothing for IDLE_TIMEOUT seconds, so a forgotten window doesn't keep a GPU
running. At most `max_concurrency` runners ever start.
"""

import asyncio
import json
import os
import socket
import subprocess
import time
from pathlib import Path

import fal
from fal.container import ContainerImage
from fastapi import WebSocket
from pydantic import BaseModel

REPO = Path(__file__).resolve().parents[2]
SERVER = "/usr/local/bin/fracst-server"
PORT = 7878
# Seconds without requests before fracst-server drops a connection.
IDLE_TIMEOUT = int(os.environ.get("FRACST_IDLE_TIMEOUT", "900"))


class Report(BaseModel):
    """A JSON report from fracst-server (--selftest or --bench)."""

    ok: bool
    report: dict


class Fracst(
    fal.App,
    name="fracst",
    keep_alive=300,
    min_concurrency=0,
    max_concurrency=1,
    max_multiplexing=8,
):
    # H100s are the cheapest GPUs with fast FP64, which the pixel loop
    # needs; fall back to an H200 or B200 when none is free. Override with
    # FRACST_FAL_MACHINE=GPU-B200 (or a comma-separated list) at deploy time.
    machine_type = os.environ.get("FRACST_FAL_MACHINE", "GPU-H100,GPU-H200,GPU-B200").split(",")
    app_auth = "private"
    # A session lasts as long as the client stays connected.
    request_timeout = 4 * 3600
    startup_timeout = 600
    image = ContainerImage.from_dockerfile(
        str(Path(__file__).with_name("Dockerfile")),
        context_dir=REPO,
        dockerignore_path=REPO / ".dockerignore",
    )

    def setup(self):
        self.server = subprocess.Popen(
            [SERVER, "--listen", f"127.0.0.1:{PORT}", "--idle-timeout", str(IDLE_TIMEOUT)]
        )
        deadline = time.time() + 120
        while time.time() < deadline:
            if self.server.poll() is not None:
                raise RuntimeError(f"fracst-server exited with code {self.server.returncode}")
            try:
                socket.create_connection(("127.0.0.1", PORT), timeout=1).close()
                return
            except OSError:
                time.sleep(0.2)
        raise TimeoutError("fracst-server did not start listening")

    @fal.endpoint("/stream", is_websocket=True)
    async def stream(self, websocket: WebSocket) -> None:
        """The fracst remote protocol, passed through to fracst-server."""
        await websocket.accept()
        await relay(websocket, PORT)

    @fal.endpoint("/selftest")
    def selftest(self) -> Report:
        """Render test views on the CPU and the GPU; they must match exactly."""
        return run_report("--selftest", timeout=1800)

    @fal.endpoint("/bench")
    def bench(self) -> Report:
        """Time the CPU and the GPU on the benchmark locations at 1920x1080."""
        return run_report("--bench", timeout=3600)


async def relay(websocket: WebSocket, port: int) -> None:
    """Pump messages both ways between `websocket` and fracst-server,
    keeping text and binary messages as they are."""
    import websockets

    async with websockets.connect(f"ws://127.0.0.1:{port}", max_size=None, ping_interval=None) as server:

        async def upstream():
            while True:
                msg = await websocket.receive()
                if msg["type"] == "websocket.disconnect":
                    return
                if msg.get("text") is not None:
                    await server.send(msg["text"])
                elif msg.get("bytes") is not None:
                    await server.send(msg["bytes"])

        async def downstream():
            async for data in server:
                if isinstance(data, str):
                    await websocket.send_text(data)
                else:
                    await websocket.send_bytes(data)
            # The server closed the connection (for example after idling).
            await websocket.close(code=server.close_code or 1000, reason=server.close_reason or "")

        tasks = [asyncio.create_task(upstream()), asyncio.create_task(downstream())]
        done, pending = await asyncio.wait(tasks, return_when=asyncio.FIRST_COMPLETED)
        for task in pending:
            task.cancel()
        for task in done:
            if task.exception() is not None and not isinstance(
                task.exception(), (websockets.ConnectionClosed, asyncio.CancelledError)
            ):
                print(f"fracst relay: {task.exception()!r}")


def run_report(flag: str, timeout: int) -> Report:
    done = subprocess.run([SERVER, flag, "--json"], capture_output=True, text=True, timeout=timeout)
    try:
        report = json.loads(done.stdout.strip().splitlines()[-1])
    except (ValueError, IndexError):
        return Report(ok=False, report={"stdout": done.stdout, "stderr": done.stderr})
    return Report(ok=done.returncode == 0, report=report)
