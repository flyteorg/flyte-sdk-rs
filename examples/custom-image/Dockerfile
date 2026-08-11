# Worker image for custom-image, built from this directory.
#
#     probe_image, rust_env = rs.rust_task(
#         crate_dir=_CRATE,
#         binary="custom-image",
#         dockerfile=_CRATE / "Dockerfile",
#     )
#
# The build context is the directory holding this file, so COPY paths are
# relative to the crate. Only the LOCAL docker builder accepts a Dockerfile --
# the remote builder takes declarative layers instead:
#
#     flyte --image-builder local run task.py probe_image --label demo
#
# Nothing requires a Dockerfile: by default the SDK declares the image as layers,
# which the remote builder compiles with no docker on your machine. Reach for
# this when the layer DSL cannot express what you need. Two such reasons appear
# below -- a system package in the runtime image (git), and a multi-stage build.

FROM rust:1-bookworm AS build
# pyo3 links libpython at build time; python3-dev provides the headers.
RUN apt-get update && apt-get install -y --no-install-recommends \
        python3 python3-dev pkg-config \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY Cargo.toml ./
COPY src ./src
ENV PYO3_PYTHON=python3
RUN cargo build --release --bin custom-image

# Multi-stage, so the Rust toolchain and the sources stay out of the shipped
# image. This is a large part of what a Dockerfile buys you today: the layer DSL
# is single-stage, so an image built from layers carries its own build inputs.
FROM debian:bookworm-slim

# The reason this example exists: `git` is what the task shells out to, and no
# declarative layer would put it in the runtime stage.
#
# libpython3.11 provides libpython3.11.so.1.0. Debian's `python3` binary links it
# statically and does not ship the shared object, so installing the interpreter
# alone leaves the worker unable to start -- flyte_core enables
# pyo3/auto-initialize, which embeds an interpreter in every task binary.
RUN apt-get update && apt-get install -y --no-install-recommends \
        git python3 libpython3.11 ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# The one hard contract between image and task: the task passes this exact path
# as args[0]. No ENTRYPOINT is needed -- Kubernetes execs the args directly --
# but setting one is fine, since the worker skips a leading token it does not
# recognise.
COPY --from=build /src/target/release/custom-image /usr/local/bin/custom-image
