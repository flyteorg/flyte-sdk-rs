# Examples

Each folder is a self-contained Rust crate (the task, which is all that runs in
the container) plus a small `task.py` that declares it to Flyte.

| Example | Shows |
|---|---|
| [`hello-trace`](hello-trace) | The basics: sequential traced steps, structs between them, and calling the task from a Python workflow ([`workflow.py`](hello-trace/workflow.py)). |
| [`concurrent-traces`](concurrent-traces) | Many traced steps in flight at once — `try_join_all` over traced fns, each recorded and replayed independently. |
| [`retry-replay`](retry-replay) | Why traces exist: an expensive step is recorded, the task fails, and the retry **replays** the step instead of re-running it. |

## Running any of them

```bash
cargo test -p <example>          # dev loop: runs the task in-process, no backend

cargo build -p <example>         # the launcher reads the interface from the binary
cd examples/<example>
flyte run task.py <task-name> --<input> <value>
```

The task names and inputs come from the Rust signatures — ask the binary:

```bash
cargo run -p <example> -- describe-interface
```

## Files in an example

| File | What it is |
|---|---|
| `src/main.rs` | The task. `#[flyte::main]` makes the crate the worker entrypoint. |
| `Cargo.toml` | A normal crate manifest depending on `flyte`. |
| `task.py` | Declares the task to Flyte: reads the interface from the binary and builds the worker image. A handful of lines via [`flyteplugins-rs`](../python/flyteplugins-rs). |
| `interface_gen.py` | Generated from `describe-interface`; the bundled interface used inside a container, where no cargo build exists. Do not edit. |

`task.py` currently adds the in-repo `flyteplugins-rs` to `sys.path`; once that
package is released, the two lines doing so go away.
