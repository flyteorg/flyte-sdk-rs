"""Generate golden values for flyte::call_task's naming/cache-key port.

Builds a real TaskSpec with the Python SDK's own serializer, then computes the
action name and cache key exactly as `RemoteController._submit` does. The Rust
test `call_task_golden` decodes the same spec and inputs and must agree.

    uv venv /tmp/pyflyte && uv pip install --python /tmp/pyflyte/bin/python flyte
    /tmp/pyflyte/bin/python -I scripts/gen_call_task_golden.py > crates/flyte/tests/golden/call_task.json
"""

import asyncio
import json
from pathlib import Path

import flyte
from flyte._internal.runtime import convert
from flyte._internal.runtime.task_serde import translate_task_to_wire
from flyte.models import ActionID, SerializationContext

env = flyte.TaskEnvironment(name="golden_env", image="ghcr.io/example/worker:abc")


@env.task(cache=flyte.Cache(behavior="override", version_override="v7", ignored_inputs=("sha",)))
async def compile(pkg: str, sha: str, jobs: int) -> str:
    return pkg


@env.task(cache="disable")
async def lint(pkg: str) -> bool:
    return True


async def case(task, kwargs, parent, seq, group):
    sctx = SerializationContext(project="p", domain="d", org="o", version="ver1", root_dir=Path.cwd())
    spec = translate_task_to_wire(task, sctx)
    inputs = await convert.convert_from_native_to_inputs(task.native_interface, **kwargs)
    proto_inputs = inputs.proto_inputs
    md = spec.task_template.metadata
    ignored = list(md.cache_ignore_input_vars)
    inputs_hash = convert.generate_inputs_hash_from_proto(proto_inputs)
    identity = convert.generate_task_identity_hash(spec.task_template)
    name_hash = convert.generate_filtered_inputs_hash(proto_inputs, ignored) if ignored else inputs_hash
    action = ActionID(name=parent, run_name="r", project="p", domain="d", org="o")
    sub = action.new_sub_action_from(task_call_seq=seq, task_hash=identity, input_hash=name_hash, group=group)
    cache_key = None
    if md.discoverable:
        cache_key = convert.generate_cache_key_hash(
            task.name, inputs_hash, spec.task_template.interface, md.discovery_version, ignored, proto_inputs
        )
    return {
        "task_spec_hex": spec.SerializeToString(deterministic=True).hex(),
        "inputs_hex": proto_inputs.SerializeToString(deterministic=True).hex(),
        "parent": parent,
        "seq": seq,
        "group": group,
        "interface_hash": convert.generate_interface_hash(spec.task_template.interface),
        "identity": identity,
        "name_inputs_hash": name_hash,
        "action_name": sub.name,
        "cache_key": cache_key,
    }


async def main():
    cases = [
        await case(compile, {"pkg": "go/pkg/a", "sha": "deadbeef", "jobs": 4}, "a0", 1, None),
        await case(compile, {"pkg": "go/pkg/a", "sha": "cafef00d", "jobs": 4}, "a0", 2, "builds"),
        await case(lint, {"pkg": "x"}, "abc123", 1, None),
    ]
    print(json.dumps(cases, indent=1))


asyncio.run(main())
