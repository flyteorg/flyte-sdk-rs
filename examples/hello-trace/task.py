"""hello-trace: three traced steps, run as a Flyte task.

    flyte run task.py my_task --x 21 --label demo

Nothing here declares the task's inputs or outputs — they come from the worker
binary itself (`cargo run -p hello-trace -- describe-interface`).
"""

from pathlib import Path

import flyteplugins_rs as rs
import interface_gen

_CRATE = Path(__file__).resolve().parent
_WORKSPACE = _CRATE.parents[1]

my_task, rust_env = rs.rust_task(
    crate_dir=_CRATE,
    binary="hello-trace",
    fallback_descriptor=interface_gen.DESCRIPTOR,
    # Path dependencies until the `flyte` crate is published; after that,
    # crate_dir alone is the whole build context.
    workspace=_WORKSPACE,
)
