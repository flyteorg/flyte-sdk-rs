"""human-approval: a deploy gated on two human decisions.

    flyte run task.py gated_deploy --version 1234

The task pauses until the approvals are signalled. Find them and answer:

    flyte get condition <run-name>
    flyte signal condition <run-name> <action-name> true

`flyte signal` takes the hashed *action* name, not the friendly condition name;
the task logs both. Give the run a generous timeout budget -- it is waiting on a
person.

Inputs and outputs come from the worker binary
(`cargo run -p human-approval -- describe-interface`), not from this file.
"""

import sys
from pathlib import Path

_CRATE = Path(__file__).resolve().parent
_WORKSPACE = _CRATE.parents[1]

# Until flyteplugins-rs is released, import it from this repo.
sys.path.insert(0, str(_WORKSPACE / "python" / "flyteplugins-rs" / "src"))

import flyteplugins_rs as rs  # noqa: E402

import interface_gen  # noqa: E402

gated_deploy, rust_env = rs.rust_task(
    crate_dir=_CRATE,
    binary="human-approval",
    fallback_descriptor=interface_gen.DESCRIPTOR,
    # Path dependencies until the `flyte` crate is published; after that,
    # crate_dir alone is the whole build context.
    workspace=_WORKSPACE,
    rs_controller=_WORKSPACE.parent / "flyte-sdk" / "rs_controller",
)
