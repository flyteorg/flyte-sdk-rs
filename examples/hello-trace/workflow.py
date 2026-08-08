"""Optional: use the Rust task from inside a Python workflow.

The Rust task becomes a child action of a Python parent — and this needs **zero
changes on the Rust side**. Calling `await my_task(...)` goes through
TaskTemplate.__call__ -> the controller, which writes the child's inputs.pb,
enqueues the action with an inline task spec, and reads back
`<outputPrefix>/outputs.pb` keyed `o0`. That is exactly what
`flyte::worker_main` already writes; nothing on that path needs a Python `func`.

Run it:

    uv run --project ../../../flyte-sdk flyte --config ~/.flyte/demo-config.yaml \
        run --project flytesnacks --domain development \
        workflow.py pipeline --x 21 --label demo

The console then shows the Python parent, the Rust task as its child action, and
that child's own three `#[flyte::trace]` children.

Note: `flyte run workflow.py` lists both `pipeline` and `my_task` as subcommands.
Task discovery is an isinstance check over module globals, and importing the Rust
task puts it in scope — harmless, and it means either can be launched from here.
"""

import flyte

# Sibling import: the CLI appends this file's directory to sys.path before
# executing it, and the default --copy-style loaded_modules carries rust_task.py
# into the parent's container.
from rust_task import my_task, rust_env

# depends_on is REQUIRED, not decorative: it is what pulls the Rust task's image
# into the plan (and therefore into the ImageCache handed to the child at
# submit time). Without it, serializing the child fails with MissingEnvironment.
# It is also sufficient — the child's task spec is built inline at enqueue, so the
# Rust env needs no separate `flyte deploy` for this to run.
env = flyte.TaskEnvironment(name="hello_trace_py", depends_on=[rust_env])


@env.task
async def pipeline(x: int = 21, label: str = "demo") -> str:
    """A Python parent that delegates the real work to the Rust task."""
    described = await my_task(x=x, label=label)
    # Prove we got a real Python str back, not a handle.
    return described.upper()

