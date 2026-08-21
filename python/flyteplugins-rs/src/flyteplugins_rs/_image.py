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


_DOCKERFILE_CONTRACT = """A worker Dockerfile has to satisfy three things (four with reuse):

0. **With `reuse=`, `unionai-actor-bridge` resolves on PATH.** A reusable task's
   pod never runs the task's own args: the fasttask plugin overwrites them with
   `unionai-actor-bridge --queue-id ... --worker-id ...`. A binary built with
   `#[union_reuse::main]` handles that argv itself, so the Dockerfile only has to
   give it the name:
   `RUN ln -sf /usr/local/bin/<binary> /usr/local/bin/unionai-actor-bridge`.
   Without it the replica exits before it can report why, and the pool shows up
   as CrashLoopBackOff with no task ever running.
1. **The binary lands at `/usr/local/bin/<binary>`.** The task passes that path
   as `args[0]`, so it is the one hard-coded contract between image and task.
2. **The image can run it.** `flyte_core` enables `pyo3/auto-initialize`, so the
   worker links `libpython` -- a slim runtime stage needs `python3` and the
   matching `libpython3.N` package, not just the interpreter.
3. **No ENTRYPOINT is required.** With none set, Kubernetes execs the args
   directly. Setting one is also fine: the worker skips a leading token it does
   not recognise.

The build context is the directory holding the Dockerfile.

Custom Dockerfiles only build with the **local** docker builder. The remote
builder rejects them outright -- it takes declarative layers, not a Dockerfile it
would have to parse -- so a config with `image: {builder: remote}` needs a local
docker and:

    flyte --image-builder local run task.py <task> ...

Note the flag sits on `flyte`, not on `run`: `flyte run --image-builder ...` is
rejected as an unknown option.

A registry is also required, because `flyte.Image.from_dockerfile` has none to
inherit -- pass `registry=` or set RUST_IMAGE_REGISTRY.

The image is named `<binary>-worker` unless `image_name=` says otherwise. Note
that pushing a name for the first time creates a *new* repository, and some
registries -- GitHub Container Registry among them -- make new packages private
by default, which the cluster then cannot pull (ImagePullBackOff, after a
successful build and push). Either make the package public once, or point
`image_name=` at a repository that already exists.
"""


def _dockerfile_image(
    *,
    dockerfile: Path,
    binary: str,
    registry: str | None,
    workspace: Path | None,
    image_name: str | None,
    reuse: bool,
    platform: str | tuple[str, ...] | None = None,
) -> flyte.Image:
    """A worker image built from a user-supplied Dockerfile."""
    if not dockerfile.is_file():
        raise FileNotFoundError(f"dockerfile {dockerfile} does not exist")
    if reuse and "unionai-actor-bridge" not in dockerfile.read_text():
        # Cheap and only a heuristic, but the failure it prevents is expensive:
        # a pool of replicas that exit instantly, with no task log to explain it.
        print(
            f"warning: {dockerfile} never mentions unionai-actor-bridge, but this task "
            f"declares reuse=. The fasttask plugin launches replicas with that command, "
            f"so the image must provide it:\n"
            f"    RUN ln -sf /usr/local/bin/{binary} /usr/local/bin/unionai-actor-bridge",
            file=sys.stderr,
        )
    if registry is None:
        raise ValueError(
            "a registry is required to build from a Dockerfile: pass registry=, or set "
            "RUST_IMAGE_REGISTRY. Unlike the layered build, flyte.Image.from_dockerfile "
            "has no registry to inherit."
        )
    if workspace is not None:
        # Silently ignoring it would be worse: the Dockerfile picks its own
        # context, so the workspace copying this argument requests never happens.
        raise ValueError(
            "workspace= and dockerfile= are mutually exclusive — a Dockerfile defines its "
            "own build context (the directory holding it), so the workspace would not be "
            "copied. Move the COPY steps into the Dockerfile and build it from a context "
            "that contains what they need."
        )
    return flyte.Image.from_dockerfile(
        file=dockerfile.resolve(),
        registry=registry,
        name=image_name or f"{binary}-worker",
        platform=platform,
    )


def _install_commands(build_dir: str, binary: str, reuse: bool) -> list[str]:
    """Build the worker and put it where the task expects to find it.

    With ``reuse``, the binary gets a second name. A reusable task's pod is not
    launched with the task's own args at all: the fasttask plugin overwrites them
    with ``unionai-actor-bridge --queue-id … --worker-id …``, so that name has to
    resolve to something on PATH or the replica crashloops before it can say why.
    A symlink is enough because the binary decides what to do from argv, and the
    `union-reuse` crate teaches it to recognise a pool launch.
    """
    # One shell, so `cd` carries and the paths after it stay relative.
    steps = [
        f"cd {build_dir}",
        f"cargo build --release --bin {binary}",
        f"install target/release/{binary} /usr/local/bin/{binary}",
    ]
    if reuse:
        steps.append(f"ln -sf /usr/local/bin/{binary} /usr/local/bin/unionai-actor-bridge")
    return [" && ".join(steps)]


def rust_worker_image(
    *,
    crate_dir: Path,
    binary: str,
    workspace: Path | None = None,
    dockerfile: Path | None = None,
    image_name: str | None = None,
    rust_base: str = "rust:1-bookworm",
    registry: str | None = None,
    reuse: bool = False,
    platform: str | tuple[str, ...] | None = None,
) -> flyte.Image:
    """An image that compiles `binary` from source and installs it in /usr/local/bin.

    Only Rust sources are copied. That matters beyond upload size: the image tag
    is a hash of each copied folder's contents, and `flyte run` itself writes
    into the working tree (`.flyte/local-cache/cache.db`), so copying a whole
    workspace makes every run mint a new tag and rebuild the image it is about
    to use.

    Normally `crate_dir` is the whole build context: cargo fetches `flyte` from
    crates.io like any other dependency. Pass `workspace` only when the crate
    cannot build alone -- a workspace member inheriting `version.workspace` or
    `[workspace.dependencies]`, or one depending on a sibling by path. That
    copies every member directory, because cargo loads all of them and a missing
    member fails before anything compiles.

    `platform` sets the architecture(s) the local docker builder targets, e.g.
    ``"linux/arm64"`` or ``("linux/amd64", "linux/arm64")`` for a multi-arch 
    manifest list Kubernetes picks from per node. Without it the image keeps
    `flyte.Image`'s default (``linux/amd64``) -- on Apple Silicon that means an
    emulated local build, which is usually what you want to override. The value
    rides on `Image.platform`, so it reaches `docker buildx build --platform`
    and the registry existence check together; the remote builder ignores it.

    `dockerfile` replaces the declarative layers entirely, for builds the layer
    DSL cannot express -- private registries, extra system libraries, a
    multi-stage build that keeps the toolchain out of the final image. See
    `_DOCKERFILE_CONTRACT` for what such a Dockerfile has to do; the repository's
    `docker/Dockerfile` is a worked example.

    `reuse` adds one step to the layered build: a second name for the binary, so
    the pod the fasttask plugin launches can find it. A Dockerfile has to add
    that line itself -- see item 0 of `_DOCKERFILE_CONTRACT`.
    """
    registry = registry if registry is not None else os.environ.get("RUST_IMAGE_REGISTRY")

    if dockerfile is not None:
        return _dockerfile_image(
            dockerfile=dockerfile,
            binary=binary,
            registry=registry,
            workspace=workspace,
            image_name=image_name,
            reuse=reuse,
            platform=platform,
        )
    context_root = workspace if workspace is not None else crate_dir
    ignore_file = _ignore_file_for(context_root)

    image = (
        flyte.Image.from_base(rust_base)
        .clone(name=image_name or f"{binary}-worker", registry=registry, extendable=True,platform=platform)
        # Replaces the SDK's default ignore set, which does not know about
        # cargo's target/ — without this the upload context is gigabytes.
        .with_dockerignore(ignore_file)
        # pyo3 build-time needs (temporary, until the pure-Rust controller
        # lands); single-stage means libpython is present at runtime for free.
        .with_apt_packages("pkg-config")
        .with_env_vars({"PYO3_PYTHON": "$UV_PYTHON"})
        # The local docker builder chowns COPY layers to a `flyte` user; the
        # remote builder chowns to the base image's runtime user. Create the user
        # so one definition builds under either.
        .with_commands(["useradd --system --create-home flyte || true"])
    )

    if workspace is None:
        # Published-crate shape: the user's crate is the entire context.
        _warn_on_unignored_target(crate_dir, ignore_file)
        return image.with_source_folder(crate_dir, "./app").with_commands(
            _install_commands("app", binary, reuse)
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

    return image.with_commands(_install_commands("ws", binary, reuse))
