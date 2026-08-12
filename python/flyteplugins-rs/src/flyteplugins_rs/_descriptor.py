"""The interface descriptor: read it from the binary, turn it into a Flyte interface.

`<binary> describe-interface` prints a one-line JSON descriptor derived by
`#[flyte::task]` from the Rust fn signature. That is the single source of truth
for a task's inputs and outputs.

Two sources, because module-level task declarations are also imported *inside* a
container (the Python-workflow path), where no cargo build exists:

- a local `cargo build` artifact, when present — authoritative, and the bundled
  copy is refreshed from it;
- otherwise the generated `_generated_interface.py` the code bundle carries.
"""

from __future__ import annotations

import importlib.util
import inspect
import json
import subprocess
import sys
from functools import cache
from pathlib import Path
from typing import Any

from flyte.models import NativeInterface

SUPPORTED_DESCRIPTOR_VERSION = 1

_GENERATED_MODULE = "_generated_interface.py"

# The descriptor's closed set of type tags -> Python types. `struct` is msgpack on
# the wire (mashumaro-compatible), which round-trips as an untyped dict.
_PY_TYPES: dict[str, type] = {
    "integer": int,
    "float": float,
    "string": str,
    "boolean": bool,
    "struct": dict,
}


def find_binary(start: Path, binary: str) -> Path | None:
    """The locally built worker binary, release preferred, or None.

    Searches `start` and its ancestors, because cargo's output directory is not
    always beside the crate: a standalone crate builds into its own `target/`,
    while a workspace member builds into the *workspace root's* `target/`, which
    is an ancestor. Checking only one layout would silently fall back to the
    bundled descriptor for the other -- losing the guarantee that the Rust
    signature is what actually gets launched.

    Bounded at the enclosing repository so a miss cannot wander up the
    filesystem and match some unrelated binary of the same name.
    """
    for directory in (start, *start.parents):
        for profile in ("release", "debug"):
            candidate = directory / "target" / profile / binary
            if candidate.is_file():
                return candidate
        if (directory / ".git").exists():
            break
    return None


@cache
def _describe(binary_path: str) -> str:
    return subprocess.run(
        [binary_path, "describe-interface"], check=True, capture_output=True, text=True
    ).stdout


def load_generated_descriptor(crate_dir: Path) -> dict[str, Any] | None:
    """The descriptor bundled beside the task, or None if there is none yet.

    Imported rather than read as text, and registered in ``sys.modules``, because
    the default ``--copy-style loaded_modules`` bundles the modules a launch
    *loaded*. A file merely opened and parsed would not travel into the
    container, and the task would then have no interface there.

    The module name is namespaced per crate: a Python workflow can pull in more
    than one Rust task, and every crate calls its generated module the same
    thing.
    """
    path = crate_dir / _GENERATED_MODULE
    if not path.is_file():
        return None

    name = f"{__package__}._generated.{crate_dir.name.replace('-', '_')}"
    spec = importlib.util.spec_from_file_location(name, path)
    if spec is None or spec.loader is None:
        return None
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return getattr(module, "DESCRIPTOR", None)


def load_descriptor(
    *,
    crate_dir: Path,
    search_from: Path,
    binary: str,
    fallback: dict[str, Any] | None = None,
) -> dict[str, Any]:
    """The worker's interface: from the binary when one is built, else bundled.

    `fallback` overrides the bundled `_generated_interface.py`, which is otherwise found
    automatically -- passing it is only necessary to declare an interface that is
    not the one sitting beside the crate.
    """
    bundled = fallback if fallback is not None else load_generated_descriptor(crate_dir)

    binary_path = find_binary(search_from, binary)
    if binary_path is None:
        if bundled is None:
            raise RuntimeError(
                f"no built {binary!r} binary and no {_GENERATED_MODULE} beside "
                f"{crate_dir} — run `cargo build --bin {binary}` once to generate it, "
                f"and commit the result so the task can also be declared where no "
                f"cargo build exists (inside a container)"
            )
        return bundled

    descriptor = json.loads(_describe(str(binary_path)))
    version = descriptor.get("flyte_interface_version")
    if version != SUPPORTED_DESCRIPTOR_VERSION:
        raise RuntimeError(
            f"{binary} speaks interface version {version}, this launcher supports "
            f"{SUPPORTED_DESCRIPTOR_VERSION} — update flyteplugins-rs or rebuild the worker"
        )

    if descriptor != bundled:
        _write_generated_module(crate_dir, descriptor)
    return descriptor


def _write_generated_module(crate_dir: Path, descriptor: dict[str, Any]) -> None:
    """Refresh the bundled copy so it tracks the Rust source.

    Kept as a Python module rather than a JSON file on purpose: the default
    `--copy-style loaded_modules` bundles imported *modules*, so a .json sitting
    beside the task would not travel into the container.
    """
    text = (
        "# Autogenerated from `<binary> describe-interface` — safe to delete, any\n"
        "# launch with a built binary writes it back. Committed so a container, which\n"
        "# has no cargo, still has an interface to declare the task from.\n"
        "DESCRIPTOR = " + repr(descriptor) + "\n"
    )
    try:
        (crate_dir / _GENERATED_MODULE).write_text(text)
        print(f"refreshed {_GENERATED_MODULE} from the worker binary")
    except OSError:
        pass  # read-only checkout: the live descriptor is still what gets used


def _py_type(var: dict[str, Any]) -> type:
    try:
        return _PY_TYPES[var["type"]]
    except KeyError:
        detail = f" ({var['detail']})" if "detail" in var else ""
        raise RuntimeError(
            f"{var['name']!r} has type {var['type']!r}, which cannot be launched "
            f"from Python yet{detail}"
        ) from None


def native_interface(descriptor: dict[str, Any]) -> NativeInterface:
    """Build the Flyte interface from a descriptor.

    Required-ness is keyed off `inspect.Parameter.empty` — the sentinel
    `NativeInterface.required_inputs()` checks. Passing None instead would make
    every input optional-with-default-None, and the container would then fail on
    a missing input rather than the CLI refusing up front.
    """
    for var in descriptor["inputs"]:
        if not var.get("required", True):
            raise RuntimeError(
                f"input {var['name']!r} declares a default, which needs a matching "
                f"literal the descriptor does not carry yet"
            )
    return NativeInterface.from_types(
        {v["name"]: (_py_type(v), inspect.Parameter.empty) for v in descriptor["inputs"]},
        {v["name"]: _py_type(v) for v in descriptor["outputs"]},
    )
