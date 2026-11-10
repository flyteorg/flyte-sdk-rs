"""fanout: a Rust task that calls a task once per value, concurrently.

    flyte run task.py sum_of_squares --n 10

The callee here is a Python task, to keep the example to one crate. A Rust
callee is declared the same way, with its own `rust_task(...)` passed in
`calls=`. Rust callees are what has been run end to end so far.
"""

from pathlib import Path

import flyte
import flyteplugins_rs as rs

_CRATE = Path(__file__).resolve().parent

env = flyte.TaskEnvironment(name="fanout_py")


@env.task
async def square(x: int) -> int:
    return x * x


# `calls=` is what makes `flyte::TaskRef::new("fanout_py.square")` resolve in
# the worker: the callee's spec is built here at deploy time and passed to the
# binary. It also adds the callee's environment to `depends_on`, so its image
# is built too, and the callee needs no separate deploy.
sum_of_squares, rust_env = rs.rust_task(
    crate_dir=_CRATE,
    binary="fanout",
    calls=[square],
)
