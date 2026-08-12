"""Fan out to the warm pool from a Python parent.

    flyte run workflow.py pipeline --n 8

This is where reuse pays: eight actions that would each have waited for a pod
land on a pool of replicas that is already running. The console shows eight
children, and their `action #N` counts show how few replicas actually served
them.

Note what the parent is **not**: reusable. A parent spends its whole life waiting
on its children, so putting it in the same pool would have it hold a slot the
children need -- with `replicas=(1, 3)` and enough parents, every slot is a
parent waiting on a child that can never start. Orchestrate from outside the
pool; do the work inside it.
"""

import asyncio

import flyte

# Sibling import: `flyte run` puts this file's directory on sys.path, and the
# default --copy-style loaded_modules carries task.py into the parent's container.
from task import rust_env, warm

# depends_on is REQUIRED, not decorative: it is what pulls the Rust task's image
# into the plan (and therefore into the ImageCache handed to the child at submit
# time). Without it, serializing the child fails with MissingEnvironment.
env = flyte.TaskEnvironment(name="reusable_py", depends_on=[rust_env])


@env.task
async def pipeline(n: int = 8) -> list[str]:
    """A non-reusable parent driving a reusable child."""
    return list(await asyncio.gather(*(warm(x=x) for x in range(1, n + 1))))
