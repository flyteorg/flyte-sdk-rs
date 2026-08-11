"""Run Rust Flyte tasks from Python.

A Rust task container runs a compiled binary and no Python. This package is the
launch-time half: it reads a worker binary's self-described interface, declares
the matching Flyte task, and builds the worker image from Rust sources — so an
example (or a user's project) needs only a few lines:

    from pathlib import Path
    import flyteplugins_rs as rs

    my_task, rust_env = rs.rust_task(
        crate_dir=Path(__file__).parent,
        binary="hello-trace",
    )

The interface is never hand-written: it comes from `<binary> describe-interface`,
so renaming a Rust parameter cannot silently diverge from what gets launched.
"""

from ._descriptor import (
    SUPPORTED_DESCRIPTOR_VERSION,
    load_descriptor,
    native_interface,
)
from ._image import rust_worker_image
from ._task import RustWorkerTask, rust_task

__all__ = [
    "SUPPORTED_DESCRIPTOR_VERSION",
    "RustWorkerTask",
    "load_descriptor",
    "native_interface",
    "rust_task",
    "rust_worker_image",
]
