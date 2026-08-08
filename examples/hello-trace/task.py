"""hello-trace: three traced steps, run as a Flyte task.

    flyte run task.py my_task --x 21 --label demo

Nothing here declares the task's inputs or outputs — they come from the worker
binary itself (`cargo run -p hello-trace -- describe-interface`).
"""

import sys
from pathlib import Path

_CRATE = Path(__file__).resolve().parent
_WORKSPACE = _CRATE.parents[1]

# Until flyteplugins-rs is released, import it from this repo.
sys.path.insert(0, str(_WORKSPACE / "python" / "flyteplugins-rs" / "src"))

import flyteplugins_rs as rs  # noqa: E402

import interface_gen  # noqa: E402

my_task, rust_env = rs.rust_task(
    crate_dir=_CRATE,
    binary="hello-trace",
    fallback_descriptor=interface_gen.DESCRIPTOR,
    # Path dependencies until the `flyte` crate is published; after that,
    # crate_dir alone is the whole build context.
    workspace=_WORKSPACE,
    rs_controller=_WORKSPACE.parent / "flyte-sdk" / "rs_controller",
)
