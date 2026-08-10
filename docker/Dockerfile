# Build context is this repo alone (see scripts/build-image.sh):
#   <ctx>/flyte-sdk-rust            (this repo)
# flyte_core comes from git (crates/flyte/Cargo.toml), so no sibling checkout
# needs to be staged into the context.
FROM rust:1-bookworm AS build
RUN apt-get update && apt-get install -y --no-install-recommends python3 python3-dev pkg-config \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY flyte-sdk-rust flyte-sdk-rust
WORKDIR /src/flyte-sdk-rust
ENV PYO3_PYTHON=python3
RUN cargo build --release -p hello-trace

# Runtime needs libpython (pyo3 auto-initialize links it) — temporary until the
# pure-Rust controller swap removes the pyo3 dependency.
FROM debian:bookworm-slim
# libpython3.11 provides libpython3.11.so.1.0 — Debian's python3 binary links it
# statically and does not ship the shared object the pyo3-linked binary needs.
RUN apt-get update && apt-get install -y --no-install-recommends python3 libpython3.11 ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/flyte-sdk-rust/target/release/hello-trace /usr/local/bin/hello-trace
ENTRYPOINT ["/usr/local/bin/hello-trace"]
