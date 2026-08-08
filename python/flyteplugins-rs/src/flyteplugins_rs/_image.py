"""The worker image, declared as Flyte image layers.

Declarative layers instead of a Dockerfile, so this works with the **remote**
image builder (`image: {builder: remote}`): no docker on your machine, and
because no layer asks for Python, the built image contains no venv — just the
Rust toolchain and the compiled worker.

Known limits, all upstream: the DSL is single-stage, so the toolchain and
sources stay in the final image; the image IDL has no ENTRYPOINT layer, so the
task puts the binary path in `args[0]`; and the remote builder ignores
`Image.platform` (you get the builder pod's architecture).
"""

from __future__ import annotations

import os
from pathlib import Path

import flyte

_IGNORE_FILE = Path(__file__).parent / "rust.dockerignore"


def rust_worker_image(
    *,
    crate_dir: Path,
    binary: str,
    workspace: Path | None = None,
    rs_controller: Path | None = None,
    rust_base: str = "rust:1-bookworm",
    registry: str | None = None,
) -> flyte.Image:
    """An image that compiles `binary` from source and installs it in /usr/local/bin.

    Only Rust sources are copied. That matters beyond upload size: the image tag
    is a hash of each copied folder's contents, and `flyte run` itself writes
    into the working tree (`.flyte/local-cache/cache.db`), so copying a whole
    workspace makes every run mint a new tag and rebuild the image it is about
    to use.

    `workspace`/`rs_controller` exist only while the `flyte` crate is
    unpublished: they carry the path dependencies in the relative layout the
    Cargo.tomls expect. Once the crate is on crates.io, `crate_dir` alone is the
    whole build context and cargo fetches the SDK like any other dependency.
    """
    registry = registry if registry is not None else os.environ.get("RUST_IMAGE_REGISTRY")

    image = (
        flyte.Image.from_base(rust_base)
        .clone(name=f"{binary}-worker", registry=registry, extendable=True)
        # Replaces the SDK's default ignore set, which does not know about
        # cargo's target/ — without this the upload context is gigabytes.
        .with_dockerignore(_IGNORE_FILE)
        # pyo3 build-time needs (temporary, until the pure-Rust controller
        # lands); single-stage means libpython is present at runtime for free.
        .with_apt_packages("python3", "python3-dev", "pkg-config")
        .with_env_vars({"PYO3_PYTHON": "python3"})
        # The local docker builder chowns COPY layers to a `flyte` user; the
        # remote builder chowns to the base image's runtime user. Create the user
        # so one definition builds under either.
        .with_commands(["useradd --system --create-home flyte || true"])
    )

    if workspace is None:
        # Published-crate shape: the user's crate is the entire context.
        return image.with_source_folder(crate_dir, "./app").with_commands(
            [
                f"cd app && cargo build --release --bin {binary}"
                f" && install target/release/{binary} /usr/local/bin/{binary}"
            ]
        )

    # Path-dependency shape: workspace manifests + crates + this example's crate.
    rel = crate_dir.relative_to(workspace)
    image = image.with_source_file(workspace / "Cargo.toml", "./ws/Cargo.toml")
    if (workspace / "Cargo.lock").exists():
        image = image.with_source_file(workspace / "Cargo.lock", "./ws/Cargo.lock")
    image = image.with_source_folder(workspace / "crates", "./ws/crates")
    image = image.with_source_file(crate_dir / "Cargo.toml", f"./ws/{rel}/Cargo.toml")
    image = image.with_source_folder(crate_dir / "src", f"./ws/{rel}/src")
    if rs_controller is not None:
        # Sibling of the workspace, matching ../../../flyte-sdk/rs_controller.
        image = image.with_source_folder(rs_controller, "./flyte-sdk/rs_controller")

    return image.with_commands(
        [
            f"cd ws && cargo build --release --bin {binary}"
            f" && install target/release/{binary} /usr/local/bin/{binary}"
        ]
    )
