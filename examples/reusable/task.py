"""reusable: the same Rust task, running in a warm container.

    flyte run task.py warm --x 7

Run it a few times. A one-shot container reports `action #1` every time; here the
count climbs, because the replica that served the last action is still alive and
serves this one too.

The only difference from any other example is `reuse=`. Everything else -- the
interface, the image, the args -- is what it always was, because a reusable task
is the same container reached a different way. In particular the image is still
built from declarative layers by the remote builder, so this needs no docker on
your machine; `reuse=` just adds one step to it, giving the binary the second
name a pool replica is launched under.
"""

from pathlib import Path

import flyte
import flyteplugins_rs as rs

_CRATE = Path(__file__).resolve().parent

warm, rust_env = rs.rust_task(
    crate_dir=_CRATE,
    binary="reusable",
    reuse=flyte.ReusePolicy(
        # (min, max): the pool scales between them. Two is the floor worth using
        # when anything fans out -- with one replica, a parent waiting on a child
        # occupies the only slot the child needs, and neither finishes.
        replicas=(1, 3),
        # Shut the whole pool down after five idle minutes. Long enough that a
        # burst of runs shares replicas; short enough not to hold nodes overnight.
        idle_ttl=300,
        # Actions per replica. Rust tasks are async, so this is free concurrency
        # for anything I/O-bound -- but they now share a process, so the total
        # memory of `concurrency` actions has to fit the replica's limit.
        concurrency=4,
    ),
)
