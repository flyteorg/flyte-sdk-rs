"""The Flyte task that launches a Rust worker binary."""

from __future__ import annotations

import base64
import json
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Sequence

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

# Past this the bundle (one container arg) is worth a warning: it is stored in
# every action of the parent and shipped to every pod.
_BUNDLE_WARN_BYTES = 256 * 1024


def spec_bundle(calls: Sequence[TaskTemplate], sctx: SerializationContext) -> str:
    """Serialize the tasks a Rust worker may call, for its ``--task-specs`` arg.

    Each spec is built exactly as Python's ``RemoteController._submit`` builds a
    child's spec at call time -- ``translate_task_to_wire`` under a context with
    no input/output paths, which the node executor fills in -- so a Rust parent
    launches the same action a Python parent would. The declared input order
    rides along: the inputs hash is order-sensitive, and the proto interface
    keeps only sorted names.

    Nesting needs nothing extra: a callee that is itself a ``RustWorkerTask``
    with ``calls`` serializes its own bundle into its own args.
    """
    # Private, but it is the one serializer whose output the backend and the
    # Python controller agree on; reimplementing it would drift.
    from flyte._internal.runtime.task_serde import translate_task_to_wire

    child_sctx = SerializationContext(
        project=sctx.project,
        domain=sctx.domain,
        org=sctx.org,
        version=sctx.version,
        code_bundle=sctx.code_bundle,
        image_cache=sctx.image_cache,
        root_dir=sctx.root_dir,
    )
    entries = [
        {
            "spec": base64.b64encode(
                translate_task_to_wire(task, child_sctx).SerializeToString(deterministic=True)
            ).decode(),
            "inputs": list(task.native_interface.inputs),
        }
        for task in calls
    ]
    bundle = base64.b64encode(json.dumps(entries, separators=(",", ":")).encode()).decode()
    if len(bundle) > _BUNDLE_WARN_BYTES:
        import warnings

        warnings.warn(f"task spec bundle for {len(calls)} callees is {len(bundle)} bytes", stacklevel=2)
    return bundle


@dataclass(kw_only=True)
class RustWorkerTask(TaskTemplate):
    """A container task whose image entrypoint is a compiled Rust worker."""

    binary: str = "worker"
    # Tasks the worker may launch with `flyte::call_task`; their specs are
    # serialized into its args at deploy time (see `spec_bundle`).
    calls: Sequence[TaskTemplate] = field(default_factory=tuple)

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
        args = [
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
        if self.calls:
            args += ["--task-specs", spec_bundle(self.calls, sctx)]
        return args


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
    platform: str | tuple[str, ...] | None = None,
    calls: Sequence[TaskTemplate] = (),
    depends_on: Sequence[flyte.Environment] = (),
    **task_kwargs: Any,
) -> tuple[RustWorkerTask, flyte.TaskEnvironment]:
    """Declare a Rust worker as a Flyte task, plus the environment holding it.

    The task's name and interface come from the binary's own descriptor, so the
    Rust signature is the only place they are written down. The descriptor is
    read from a local `cargo build` when there is one, and otherwise from the
    generated ``_generated_interface.py`` beside the crate, which is found
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

    Pass ``calls`` to let the worker launch other tasks with
    ``flyte::call_task``: each callee's spec is built at deploy time and handed
    to the worker, and the callee's environment is added to ``depends_on`` so
    its image is built with this one. Callees need no separate deployment, and
    may be Rust or Python tasks. ``depends_on`` adds further environments.

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
            platform=platform
        ),
        interface=native_interface(descriptor),
        reusable=reuse,
        calls=tuple(calls),
        **task_kwargs,
    )
    deps = list(depends_on)
    for callee in calls:
        callee_env = callee.parent_env() if callee.parent_env else None
        if callee_env is None:
            raise ValueError(f"task {callee.name} called by {task.name} belongs to no environment")
        if all(callee_env is not d for d in deps):
            deps.append(callee_env)
    env = flyte.TaskEnvironment.from_task(
        env_name or f"{binary.replace('-', '_')}_env", task, depends_on=deps or None
    )
    # `from_task` has no `reusable` parameter, and the field is what the SDK reads
    # off the *task* when serializing. Setting it on the env too keeps the pair
    # consistent for anything that inspects the environment instead.
    if reuse is not None:
        object.__setattr__(env, "reusable", reuse)
    return task, env
