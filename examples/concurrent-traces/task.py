"""concurrent-traces: n traced steps in flight at once.

    flyte run task.py fanout --n 8

Each value gets its own trace action, recorded and replayed independently.

Inputs and outputs come from the worker binary
(`cargo run -p concurrent-traces -- describe-interface`), not from this file.
"""

import sys
from pathlib import Path

_CRATE = Path(__file__).resolve().parent
_WORKSPACE = _CRATE.parents[1]

# Until flyteplugins-rs is released, import it from this repo.
sys.path.insert(0, str(_WORKSPACE / "python" / "flyteplugins-rs" / "src"))

import flyteplugins_rs as rs  # noqa: E402

import interface_gen  # noqa: E402

fanout, rust_env = rs.rust_task(
    crate_dir=_CRATE,
    binary="concurrent-traces",
    fallback_descriptor=interface_gen.DESCRIPTOR,
    # Path dependencies until the `flyte` crate is published; after that,
    # crate_dir alone is the whole build context.
    workspace=_WORKSPACE,
)
