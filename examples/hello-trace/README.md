# hello-trace

One example, two languages — and only one of them runs in the task container.

| File | Language | Runs where |
|---|---|---|
| `src/main.rs` | Rust | **In the task container.** The whole task: three `#[flyte::trace]` steps and the `#[flyte::main]` entrypoint. |
| `rust_task.py` | Python | **Launch-time only.** Declares the task (image + interface) for the control plane. Goes away when the native Rust launcher lands. |
| `workflow.py` | Python | Optional. A Python parent task that calls the Rust task as a child action. |

Not part of the example: `src/bin/smoke_fixture.rs` (fixtures for
`scripts/smoke.sh`) and `interface_gen.py` (generated, see below).

## Dev loop — no backend, no container

```bash
cargo test -p hello-trace          # runs the task in-process via flyte::run
```

Traced steps just execute their bodies when no backend is attached.

## Launch it

```bash
cargo build -p hello-trace         # once: rust_task.py reads the interface from the binary

flyte run rust_task.py my_task --x 21 --label demo
```

Nothing to configure: the worker image is declared as `flyte.Image` layers in
`rust_worker_image()` and built by Flyte's image builder on the first run — no
Dockerfile, no docker on your machine, no push. Later runs reuse it. Because no
layer asks for Python, the built image contains no venv.

Prefer a small hand-optimized image? `scripts/build-image.sh` builds a
multi-stage one (~147MB vs ~1.5GB single-stage); swap the task's image for
`flyte.Image.from_base("<pushed-uri>")`.

## The interface is generated, not declared

```bash
cargo run -p hello-trace -- describe-interface
# {"flyte_interface_version":1,"task":"my_task","inputs":[...],"outputs":[...]}
```

`rust_task.py` builds its `NativeInterface` from exactly this, so the Rust
signature is the single source of truth — rename a parameter and the launcher
follows. On a dev machine it reads the binary live and refreshes
`interface_gen.py` when stale; inside a task container (the workflow path
imports this module in the parent's container, where there is no cargo build)
the generated module stands in.

## From a Python workflow

```bash
flyte run workflow.py pipeline --x 21 --label demo
```

The console shows the Python parent, the Rust task as its child action, and that
child's own three trace children. `depends_on=[rust_env]` on the parent
environment is required — it is what puts the Rust image in the plan.

`--local` is not supported for the Rust task in either path: local mode would
have to run the container itself.
