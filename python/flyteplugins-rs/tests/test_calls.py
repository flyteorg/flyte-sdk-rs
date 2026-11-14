"""rust_task(calls=[...]): the spec bundle a Rust worker launches children from."""

import base64
import json
from pathlib import Path

import flyte
from flyte._internal.runtime.task_serde import translate_task_to_wire
from flyte.models import SerializationContext

from flyteplugins_rs import rust_task

IMAGE = flyte.Image.from_base("ghcr.io/example/worker:abc")


def _descriptor(task, inputs):
    return {
        "flyte_interface_version": 1,
        "task": task,
        # Deliberately not sorted: the declared order must survive.
        "inputs": [{"name": n, "type": "string", "required": True} for n in inputs],
        "outputs": [{"name": "o0", "type": "string"}],
    }


def _task(name, inputs, **kw):
    return rust_task(
        crate_dir=Path("/nonexistent"),
        binary=name,
        fallback_descriptor=_descriptor(name, inputs),
        image=IMAGE,
        **kw,
    )


def _sctx():
    return SerializationContext(
        project="p", domain="d", org="o", version="v1", root_dir=Path.cwd()
    )


def _bundle(spec):
    args = list(spec.task_template.container.args)
    assert "--task-specs" in args
    bundle = json.loads(base64.b64decode(args[args.index("--task-specs") + 1]))
    assert bundle["v"] == 1
    return bundle["tasks"]


def test_bundle_holds_the_spec_python_would_submit():
    leaf, leaf_env = _task(
        "compile",
        ["sha", "pkg"],
        cache=flyte.Cache(behavior="override", version_override="v1"),
    )
    parent, parent_env = _task("graph", ["sha"], calls=[leaf])

    assert leaf_env in parent_env.depends_on

    (entry,) = _bundle(translate_task_to_wire(parent, _sctx()))
    assert entry["inputs"] == ["sha", "pkg"]
    # What RemoteController._submit would build for this child at call time.
    expected = translate_task_to_wire(
        leaf,
        SerializationContext(
            project="p", domain="d", org="o", version="v1", root_dir=Path.cwd()
        ),
    )
    assert base64.b64decode(entry["spec"]) == expected.SerializeToString(
        deterministic=True
    )


def test_nesting_is_transitive():
    leaf, _ = _task("test", ["pkg"])
    mid, _ = _task("unit_tests", ["sha"], calls=[leaf])
    top, _ = _task("pr", ["sha"], calls=[mid])

    (mid_entry,) = _bundle(translate_task_to_wire(top, _sctx()))
    from flyteidl2.task import task_definition_pb2

    mid_spec = task_definition_pb2.TaskSpec.FromString(
        base64.b64decode(mid_entry["spec"])
    )
    (leaf_entry,) = _bundle(mid_spec)
    assert leaf_entry["inputs"] == ["pkg"]


def test_no_calls_no_arg():
    t, _ = _task("plain", ["x"])
    assert (
        "--task-specs"
        not in translate_task_to_wire(t, _sctx()).task_template.container.args
    )


def test_callee_is_found_by_env_qualified_name():
    # flyte::TaskRef::new("<env>.<task>") resolves against these two fields: a
    # rust_task's own name stays bare (it is what the backend registers).
    from flyteidl2.task import task_definition_pb2

    leaf, _ = _task("step_rs", ["x"], env_name="pr-shadow-rs-step")
    parent, _ = _task("pr_rs", ["sha"], calls=[leaf])
    (entry,) = _bundle(translate_task_to_wire(parent, _sctx()))
    spec = task_definition_pb2.TaskSpec.FromString(base64.b64decode(entry["spec"]))
    assert (spec.environment.name, spec.task_template.id.name) == (
        "pr-shadow-rs-step",
        "step_rs",
    )
