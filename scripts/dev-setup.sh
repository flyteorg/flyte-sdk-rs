#!/usr/bin/env bash
# Point the launcher at this checkout instead of the released package.
#
# The examples import `flyteplugins_rs` like any user would, so by default they
# run against whatever is installed from PyPI. That means edits under
# python/flyteplugins-rs/src have no effect until you install the working tree
# over it -- which is what this does:
#
#   ./scripts/dev-setup.sh
#
# An editable install, so further edits take effect with no reinstall. To go
# back to the released package:
#
#   uv pip install --force-reinstall flyteplugins-rs
#
# The Rust half is separate: examples take `flyte` by path within this
# workspace, so they always build the working tree. To point a crate OUTSIDE
# this repo at a local checkout, add to its Cargo.toml:
#
#   [patch.crates-io]
#   flyte = { path = "/path/to/flyte-sdk-rs/crates/flyte" }
set -euo pipefail
cd "$(dirname "$0")/.."

if ! command -v uv >/dev/null 2>&1; then
    echo "error: uv not found — see https://docs.astral.sh/uv/getting-started/installation/" >&2
    exit 1
fi

uv pip install -e python/flyteplugins-rs

python - <<'PY'
import pathlib

import flyteplugins_rs as rs

src = pathlib.Path(rs.__file__).resolve()
repo = pathlib.Path.cwd().resolve()
where = "this checkout" if repo in src.parents else "the installed package"
print(f"\nflyteplugins_rs -> {src.parent}  ({where})")
PY
