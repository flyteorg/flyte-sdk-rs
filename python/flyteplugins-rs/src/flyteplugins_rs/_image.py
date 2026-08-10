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
import sys
from pathlib import Path

import flyte

# Fallback only. The tag hash can only honour an ignore file that is named
# `.dockerignore` and sits at an ancestor of the copied sources -- see
# `_ignore_file_for` -- which a file shipped inside this package never is.
_PACKAGED_IGNORE_FILE = Path(__file__).parent / "rust.dockerignore"

_IGNORE_NAME = ".dockerignore"


def _ignore_file_for(context_root: Path) -> Path:
    """The ignore file to declare, preferring one the tag hash can actually use.

    The SDK consumes the ignore list through two different paths, and only one of
    them accepts an arbitrary filename:

    - the build-context upload reads the DockerIgnore layer's path directly, so
      any filename works;
    - `Image._get_hash_digest` (which computes the tag) instead constructs a
      `DockerfileIgnore(<dir of that path>)`, and that class loads only
      `<dir>/.dockerignore` and only matches paths *underneath* `<dir>`.

    So an ignore file named anything else, or living outside the copied sources,
    is silently a no-op for the tag: the hash reads every "ignored" file --
    cargo's multi-GB `target/` included -- and any launcher edit mints a new tag
    and triggers a full image rebuild. Preferring `<context_root>/.dockerignore`
    satisfies both paths at once.
    """
    candidate = context_root / _IGNORE_NAME
    if candidate.is_file():
        return candidate

    print(
        f"warning: no {_IGNORE_NAME} at {context_root}; falling back to "
        f"{_PACKAGED_IGNORE_FILE.name}, which the SDK cannot apply when hashing the "
        f"image tag. Expect slow `flyte run` startup (the hash reads target/) and a "
        f"rebuild on every source edit. Fix: copy {_PACKAGED_IGNORE_FILE} to "
        f"{candidate}.",
        file=sys.stderr,
    )
    return _PACKAGED_IGNORE_FILE


def _warn_on_unignored_target(source: Path, ignore_file: Path) -> None:
    """Flag a copied folder whose `target/` the tag hash will read anyway.

    Hashing a cargo `target/` is a silent multi-second-to-multi-minute stall with
    no output, so name the folder rather than letting `flyte run` look hung.
    """
    if not (source / "target").is_dir():
        return
    if ignore_file.name == _IGNORE_NAME and ignore_file.parent in (source, *source.parents):
        return  # the hash will apply the ignore to this folder
    print(
        f"warning: {source} contains a cargo target/ that {ignore_file} does not cover "
        f"when the image tag is hashed. `flyte run` will read that whole directory "
        f"before it launches, and any local cargo build will change the image tag.",
        file=sys.stderr,
    )


def rust_worker_image(
    *,
    crate_dir: Path,
    binary: str,
    workspace: Path | None = None,
    rust_base: str = "rust:1-bookworm",
    registry: str | None = None,
) -> flyte.Image:
    """An image that compiles `binary` from source and installs it in /usr/local/bin.

    Only Rust sources are copied. That matters beyond upload size: the image tag
    is a hash of each copied folder's contents, and `flyte run` itself writes
    into the working tree (`.flyte/local-cache/cache.db`), so copying a whole
    workspace makes every run mint a new tag and rebuild the image it is about
    to use.

    `workspace` exists only while the `flyte` crate is unpublished: it carries
    the sibling path dependencies (`crates/flyte`, `crates/flyte-macros`) in the
    relative layout the Cargo.tomls expect. Once the crate is on crates.io,
    `crate_dir` alone is the whole build context and cargo fetches the SDK like
    any other dependency.
    """
    registry = registry if registry is not None else os.environ.get("RUST_IMAGE_REGISTRY")
    context_root = workspace if workspace is not None else crate_dir
    ignore_file = _ignore_file_for(context_root)

    image = (
        flyte.Image.from_base(rust_base)
        .clone(name=f"{binary}-worker", registry=registry, extendable=True)
        # Replaces the SDK's default ignore set, which does not know about
        # cargo's target/ — without this the upload context is gigabytes.
        .with_dockerignore(ignore_file)
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
        _warn_on_unignored_target(crate_dir, ignore_file)
        return image.with_source_folder(crate_dir, "./app").with_commands(
            [
                (
                    f"cd app && cargo build --release --bin {binary}"
                    f" && install target/release/{binary} /usr/local/bin/{binary}"
                )
            ]
        )

    # Path-dependency shape: the whole workspace's Rust sources.
    #
    # Every workspace member must be present even when only one is being built --
    # cargo loads all of them, and a missing member fails before compiling
    # anything. So copy the member directories wholesale rather than just this
    # crate's. Python is excluded by the dockerignore above, which is what keeps
    # launcher edits from changing the image tag.
    image = image.with_source_file(workspace / "Cargo.toml", "./ws/Cargo.toml")
    if (workspace / "Cargo.lock").exists():
        image = image.with_source_file(workspace / "Cargo.lock", "./ws/Cargo.lock")
    for member_dir in ("crates", "examples"):
        if (workspace / member_dir).is_dir():
            _warn_on_unignored_target(workspace / member_dir, ignore_file)
            image = image.with_source_folder(workspace / member_dir, f"./ws/{member_dir}")

    return image.with_commands(
        [
            (
                f"cd ws && cargo build --release --bin {binary}"
                f" && install target/release/{binary} /usr/local/bin/{binary}"
            )
        ]
    )
