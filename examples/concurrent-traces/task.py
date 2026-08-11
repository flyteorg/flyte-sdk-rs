"""concurrent-traces: n traced steps in flight at once.

    flyte run task.py fanout --n 8

Each value gets its own trace action, recorded and replayed independently.

Inputs and outputs come from the worker binary
(`cargo run -p concurrent-traces -- describe-interface`), not from this file.
"""

from pathlib import Path

import flyteplugins_rs as rs

_CRATE = Path(__file__).resolve().parent

fanout, rust_env = rs.rust_task(
    crate_dir=_CRATE,
    binary="concurrent-traces",
)
