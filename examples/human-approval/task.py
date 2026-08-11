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

from pathlib import Path

import flyteplugins_rs as rs
import interface_gen

_CRATE = Path(__file__).resolve().parent

gated_deploy, rust_env = rs.rust_task(
    crate_dir=_CRATE,
    binary="human-approval",
    fallback_descriptor=interface_gen.DESCRIPTOR,
)
