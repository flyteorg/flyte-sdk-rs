"""The Flyte task that launches a Rust worker binary."""

from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path
from typing import Any

import flyte
from flyte.extend import TaskTemplate
from flyte.models import SerializationContext

from ._descriptor import load_descriptor, native_interface
from ._image import rust_worker_image
from ._reuse import ACTOR_TASK_TYPE, actor_custom_config
from ._reuse import validate as validate_reuse

# Not "python" (nothing python runs here) and never "raw-container" (that one
# injects the copilot data sidecar). The leaseworker resolves an unregistered
# type to the default pod plugin, which is the plain container handling we want.
RUST_TASK_TYPE = "rust-task"


@dataclass(kw_only=True)
class RustWorkerTask(TaskTemplate):
    """A container task whose image entrypoint is a compiled Rust worker."""

    binary: str = "worker"

    async def execute(self, *args, **kwargs):
        # Only reached under mode="local" / `flyte run --local`, which would mean
        # running the container ourselves (see flyte.extras.ContainerTask).
        raise NotImplementedError("remote-only task; --local is not supported")

    def custom_config(self, sctx: SerializationContext) -> dict[str, Any]:
        # Only reusable tasks carry a `custom`, and for them it is load-bearing:
        # the fasttask plugin reads the environment's shape (replicas, TTLs,
        # parallelism, identity) out of exactly this blob. See `_reuse` for why
        # we build it here rather than letting the SDK do it.
        if self.reusable is None:
            return {}
        return actor_custom_config(self, sctx, self.reusable)

    def container_args(self, sctx: SerializationContext) -> list[str]:
        # Mirrors flyte._task.AsyncFunctionTaskTemplate.container_args, trimmed to
        # what the Rust worker's arg parser consumes.
        #
        # - args[0] is the binary: images built from image layers have no
        #   ENTRYPOINT (the image IDL has no such layer) and the container
        #   command is empty, so k8s execs args directly. Under an image that
        #   does set an ENTRYPOINT this token is just an unrecognized arg the
        #   worker skips — one arg list, valid either way.
        # - "a0" is the leading token the Python runtime also emits; the worker
        #   skips it (it names a click subcommand there, nothing here).
        # - input/output paths keep their {{.input}} / {{.outputPrefix}}
        #   defaults, which the backend substitutes per action. This is also what
        #   lets the task run as a CHILD action: the controller deliberately
        #   leaves them unset for the node executor to fill in.
        # - {{.runName}} / {{.actionName}} are NOT in the backend's substitution
        #   set. The worker discards any "{{...}}" value and takes those names
        #   from the RUN_NAME / ACTION_NAME env the backend injects; they stay as
        #   forward-compatible placeholders.
        # - --run-base-dir is never an arg; it arrives only as _U_RUN_BASE.
        return [
            f"/usr/local/bin/{self.binary}",
            "a0",
            "--inputs",
            sctx.input_path,
            "--outputs-path",
            sctx.output_path,
            "--run-name",
            "{{.runName}}",
            "--name",
            "{{.actionName}}",
        ]


def rust_task(
    *,
    crate_dir: Path,
    binary: str,
    fallback_descriptor: dict[str, Any] | None = None,
    workspace: Path | None = None,
    dockerfile: Path | None = None,
    image_name: str | None = None,
    env_name: str | None = None,
    image: flyte.Image | None = None,
    reuse: flyte.ReusePolicy | None = None,
    **task_kwargs: Any,
) -> tuple[RustWorkerTask, flyte.TaskEnvironment]:
    """Declare a Rust worker as a Flyte task, plus the environment holding it.

    The task's name and interface come from the binary's own descriptor, so the
    Rust signature is the only place they are written down. The descriptor is
    read from a local `cargo build` when there is one, and otherwise from the
    generated ``interface_gen.py`` beside the crate, which is found
    automatically; ``fallback_descriptor`` only needs passing to override that.

    The worker image is built from declarative layers by default and named
    ``<binary>-worker``; ``image_name`` overrides that, which is how you push to
    a repository that already exists rather than creating one. Pass ``dockerfile``
    to supply your own build instead -- see :func:`rust_worker_image` for the
    contract it has to meet -- or ``image`` for a fully custom ``flyte.Image``.

    Pass ``reuse`` to run the task in a **warm container**: the backend keeps a
    pool of replicas alive and streams actions to them instead of scheduling a
    pod each time, which trades pod startup for a process that outlives any one
    action. This needs the Rust side to opt in too — the crate depends on
    ``union-reuse`` and the task fn carries ``#[union_reuse::main]`` instead of
    ``#[flyte::main]``. A binary built that way still runs fine without
    ``reuse``, so the two halves can be changed in either order.

    Extra keyword arguments (``retries``, ``cache``, ``resources``, ``timeout``,
    ...) pass straight through to the underlying task template.

    Returns ``(task, env)``. The env matters for composition: a Python parent
    calling this task needs ``depends_on=[env]`` so the worker image is carried
    into the deployment plan.
    """
    if reuse is not None:
        validate_reuse(reuse)

    descriptor = load_descriptor(
        # Where to start looking for the built binary. `find_binary` walks up
        # from here, so a workspace member resolves to the workspace's target/
        # without `workspace` having to be passed.
        crate_dir=crate_dir,
        search_from=crate_dir,
        binary=binary,
        fallback=fallback_descriptor,
    )

    task = RustWorkerTask(
        name=descriptor["task"],
        binary=binary,
        # The task type is what routes a task to a backend plugin, so reuse has
        # to change it: `rust-task` lands on the default pod plugin (one pod per
        # action), `actor` on the fasttask plugin that owns replica pools.
        task_type=ACTOR_TASK_TYPE if reuse is not None else RUST_TASK_TYPE,
        image=image
        or rust_worker_image(
            crate_dir=crate_dir,
            binary=binary,
            workspace=workspace,
            dockerfile=dockerfile,
            image_name=image_name,
            reuse=reuse is not None,
        ),
        interface=native_interface(descriptor),
        reusable=reuse,
        **task_kwargs,
    )
    env = flyte.TaskEnvironment.from_task(env_name or f"{binary.replace('-', '_')}_env", task)
    # `from_task` has no `reusable` parameter, and the field is what the SDK reads
    # off the *task* when serializing. Setting it on the env too keeps the pair
    # consistent for anything that inspects the environment instead.
    if reuse is not None:
        object.__setattr__(env, "reusable", reuse)
    return task, env
