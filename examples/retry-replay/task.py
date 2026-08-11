"""retry-replay: an expensive step is replayed instead of re-run on retry.

    flyte run task.py flaky --seed 6

The task fails on its first attempt on purpose. `retries` below is what lets
Flyte try again; on that attempt the recorded `slow_step` is replayed, so the
2s of work does not happen twice. Watch the logs for
"replaying recorded trace".

Inputs and outputs come from the worker binary
(`cargo run -p retry-replay -- describe-interface`), not from this file.
"""

from pathlib import Path

import flyteplugins_rs as rs

_CRATE = Path(__file__).resolve().parent

flaky, rust_env = rs.rust_task(
    crate_dir=_CRATE,
    binary="retry-replay",
    # Without this the deliberate first-attempt failure is just a failure.
    retries=2,
)
