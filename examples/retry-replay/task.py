"""retry-replay: an expensive step is replayed instead of re-run on retry.

    flyte run task.py flaky --seed 6

The task fails on its first attempt on purpose. `retries` below is what lets
Flyte try again; on that attempt the recorded `slow_step` is replayed, so the
2s of work does not happen twice. Watch the logs for
"replaying recorded trace".

Inputs and outputs come from the worker binary
(`cargo run -p retry-replay -- describe-interface`), not from this file.
"""

import sys
from pathlib import Path

_CRATE = Path(__file__).resolve().parent
_WORKSPACE = _CRATE.parents[1]

# Until flyteplugins-rs is released, import it from this repo.
sys.path.insert(0, str(_WORKSPACE / "python" / "flyteplugins-rs" / "src"))

import flyteplugins_rs as rs  # noqa: E402

import interface_gen  # noqa: E402

flaky, rust_env = rs.rust_task(
    crate_dir=_CRATE,
    binary="retry-replay",
    fallback_descriptor=interface_gen.DESCRIPTOR,
    # Without this the deliberate first-attempt failure is just a failure.
    retries=2,
    # Path dependencies until the `flyte` crate is published; after that,
    # crate_dir alone is the whole build context.
    workspace=_WORKSPACE,
)
