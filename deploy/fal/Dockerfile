# fracst-server for fal.ai, or any machine with an NVIDIA GPU. From the
# repository root:
#   docker build -f deploy/fal/Dockerfile -t fracst-server .
#   docker run --gpus all -p 7878:7878 fracst-server fracst-server --listen 0.0.0.0:7878 --token SECRET

FROM rust:1-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock rustfmt.toml ./
COPY crates ./crates
COPY assets ./assets
RUN cargo build --release --locked -p fracst-server

# Python because fal runs the app (deploy/fal/app.py) inside the image; the
# version matches the Python that deploys it, which pickles the app.
FROM python:3.11-slim-bookworm
# fal needs curl at start-up.
RUN apt-get update \
    && apt-get install -y --no-install-recommends curl ca-certificates \
    && rm -rf /var/lib/apt/lists/*
# NVRTC compiles the kernels for whichever GPU the container lands on. The
# driver (libcuda) comes from the host through the NVIDIA container runtime.
RUN pip install --no-cache-dir "nvidia-cuda-nvrtc-cu12==12.9.*" "websockets>=13,<16"
ENV NVIDIA_VISIBLE_DEVICES=all \
    NVIDIA_DRIVER_CAPABILITIES=compute,utility \
    LD_LIBRARY_PATH=/usr/local/lib/python3.11/site-packages/nvidia/cuda_nvrtc/lib
COPY --from=build /src/target/release/fracst-server /usr/local/bin/fracst-server
# Last, so fal's own dependency versions win.
RUN pip install --no-cache-dir fal
