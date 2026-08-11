"""custom-image: a worker image built from a Dockerfile instead of layers.

    export RUST_IMAGE_REGISTRY=<your-registry>
    flyte run --image-builder local task.py probe_image --label demo

Two things this example needs that the others do not:

- `--image-builder local`, because a Dockerfile only builds with the local docker
  builder. The remote builder takes declarative layers, which is what every other
  example uses and what needs no docker on your machine.
- a registry, because `flyte.Image.from_dockerfile` has none to inherit. Set
  `RUST_IMAGE_REGISTRY`, or pass `registry=` to `rust_task`.

The task calls `git`, which the base image does not ship — so it runs only if the
image really was built from this crate's Dockerfile.
"""

from pathlib import Path

import flyteplugins_rs as rs

_CRATE = Path(__file__).resolve().parent

probe_image, rust_env = rs.rust_task(
    crate_dir=_CRATE,
    binary="custom-image",
    # Replaces the declarative layers entirely. The build context is _CRATE,
    # the directory holding the Dockerfile.
    dockerfile=_CRATE / "Dockerfile",
)
