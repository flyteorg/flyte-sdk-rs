# flyteplugins-rs

> [!WARNING]
> **Experimental.** The APIs and wire contracts may change without notice.

Launch [Rust Flyte tasks](https://github.com/flyteorg/flyte-sdk-rs) from Python.

A Rust task container runs a compiled binary and no Python. This package is the
launch-time half: it reads a worker binary's self-described interface, declares
the matching Flyte task, and builds the worker image from Rust sources.

```bash
pip install flyteplugins-rs
```

```python
from pathlib import Path

import flyteplugins_rs as rs

my_task, rust_env = rs.rust_task(
    crate_dir=Path(__file__).parent,
    binary="hello-trace",
    retries=2,  # any TaskTemplate option passes through
)
```

```bash
flyte run task.py my_task --x 21 --label demo
```

The task's name, inputs, and outputs are never written here — they come from the
Rust fn signature, so renaming a parameter cannot silently diverge from what gets
launched.

`rust_env` is the environment holding the task. A Python parent calling it needs
`depends_on=[rust_env]`, which carries the worker image into the deployment plan:

```python
env = flyte.TaskEnvironment(name="my_pipeline", depends_on=[rust_env])

@env.task
async def pipeline(x: int) -> str:
    return await my_task(x=x, label="demo")
```

## What it does

- **Interface** — runs `<binary> describe-interface` when a local build exists,
  and refreshes the generated `interface_gen.py` from it. Inside a container
  (no cargo build) the generated module is used instead.
- **Image** — declares the worker image as `flyte.Image` layers, so the
  **remote** image builder can compile it (no Dockerfile, no local docker). No
  layer asks for Python, so the image gets no venv.
- **Task** — a container task typed `rust-task`, whose args match the worker's
  contract, and whose `args[0]` is the binary (image layers cannot set an
  ENTRYPOINT).

## Bringing your own Dockerfile

By default the worker image is declared as `flyte.Image` layers, which is what
lets the **remote** builder compile it with no docker on your machine. When that
is not enough — a private base image, extra system libraries, a multi-stage build
that keeps the Rust toolchain out of the final image — pass a Dockerfile instead:

```python
my_task, rust_env = rs.rust_task(
    crate_dir=Path(__file__).parent,
    binary="hello-trace",
    dockerfile=Path(__file__).parent / "Dockerfile",
)
```

Three things such a Dockerfile has to get right:

1. **Install the binary at `/usr/local/bin/<binary>`.** The task passes that
   exact path as `args[0]`; it is the one hard-coded contract between the image
   and the task.
2. **Ship `libpython`.** `flyte_core` enables `pyo3/auto-initialize`, so the
   worker links against it. A slim runtime stage needs the `libpython3.N`
   package, not just the `python3` interpreter — Debian's `python3` links it
   statically and does not provide the shared object.
3. **An `ENTRYPOINT` is optional.** With none set, Kubernetes execs the args
   directly. Setting one is fine too: the worker skips a leading token it does
   not recognise, so one arg list works either way.

The build context is the directory holding the Dockerfile, so `COPY` paths are
relative to it. `workspace=` and `dockerfile=` are mutually exclusive — a
Dockerfile brings its own context, so put the copying in the Dockerfile.

> [!IMPORTANT]
> Custom Dockerfiles build only with the **local** docker builder. The remote
> builder takes declarative layers, not a Dockerfile it would have to parse, and
> rejects one outright. With `image: {builder: remote}` in your config, this path
> needs `flyte run --image-builder local` and a working local docker.

A worked example is the
[`custom-image`](https://github.com/flyteorg/flyte-sdk-rs/tree/main/examples/custom-image)
example: multi-stage so the toolchain does not ship, and installing a system
package the task actually depends on.

One extra requirement: `flyte.Image.from_dockerfile` has no registry to inherit,
so set `RUST_IMAGE_REGISTRY` or pass `registry=` to `rust_task`.

## Compatibility

The launcher and the worker agree on a versioned descriptor contract, currently
`flyte_interface_version = 1`. A worker built against a newer SDK is refused at
launch with a message naming both sides, rather than failing inside the
container. Package versions are deliberately **not** kept in step with the
`flyte` Rust crate's — see
[docs/releasing.md](https://github.com/flyteorg/flyte-sdk-rs/blob/main/docs/releasing.md).

## Developing

To run against a checkout instead of the released package:

```bash
git clone https://github.com/flyteorg/flyte-sdk-rs && cd flyte-sdk-rs
./scripts/dev-setup.sh
```
