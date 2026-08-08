> [!WARNING]
> **Experimental.** This SDK is a working preview: the APIs, the wire
> contracts, and the internals may all change without notice. Feedback and
> issues are very welcome.

# Flyte Rust SDK

**Write [Flyte](https://github.com/flyteorg/flyte-sdk) tasks in Rust — traced,
replayed on retry, and composable with Python workflows.**

Mark steps of an async fn with `#[flyte::trace]`: each step is recorded as a
child action on the Flyte backend (visible in the console) and **replayed
instead of re-run when the task retries** — in-process checkpointing.
Primitives and serde structs (msgpack, wire-compatible with Python dataclasses)
pass between steps.

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, flyte::FlyteStruct)]
struct Stats { mean: f64, count: i64, label: String }

#[flyte::trace]
async fn double(x: i64) -> Result<i64, flyte::Error> {
    Ok(x * 2)
}

#[flyte::trace]
async fn compute_stats(total: i64, label: String) -> Result<Stats, flyte::Error> {
    Ok(Stats { mean: total as f64 / 2.0, count: 2, label })
}

#[flyte::main]   // generates fn main(): this crate IS the task container
#[flyte::task]
async fn my_task(x: i64, label: String) -> Result<String, flyte::Error> {
    let stats = compute_stats(double(x).await?, label).await?;
    Ok(format!("{}: mean={} over {} values", stats.label, stats.mean, stats.count))
}
```

That is the whole program — no `fn main`, no worker plumbing. The full example
lives in [`examples/hello-trace`](examples/hello-trace); see
[`examples/`](examples) for concurrent traces and replay-on-retry.

## Run locally

Without a backend attached, traced fns simply run their bodies, so `flyte::run`
executes the whole task in-process — write your dev loop as a plain test:

```rust
#[test]
fn runs_locally() {
    let out = flyte::run(my_task(21, "demo".to_string())).unwrap();
    assert_eq!(out, "demo: mean=21 over 2 values");
}
```

```bash
cargo test -p hello-trace
```

## Run on a Flyte backend

The binary describes its own interface — the Rust signature is the only place
the inputs and outputs are declared:

```bash
cargo build -p hello-trace
cargo run -p hello-trace -- describe-interface
# {"flyte_interface_version":1,"task":"my_task",
#  "inputs":[{"name":"x","type":"integer","required":true}, ...],
#  "outputs":[{"name":"o0","type":"string"}]}
```

`task.py` reads exactly that to register the task — a few lines via
[`flyteplugins-rs`](python/flyteplugins-rs) — and Flyte's remote image builder
builds the worker image on the first run (no Dockerfile, no docker on your
machine, cached afterwards). Make sure your config uses the remote builder:

```yaml
# ~/.flyte/config.yaml
admin:
  endpoint: dns:///<your-flyte-endpoint>
image:
  builder: remote
```

```bash
cd examples/hello-trace
flyte run task.py my_task --x 21 --label demo
# o0: "demo: mean=21 over 2 values"
```

The console shows the run with one child trace action per `#[flyte::trace]`
step, recorded by the Rust SDK from inside the container.

## Use it from a Python workflow

A Python task can call the Rust task as a child action — no Rust-side changes:

```python
import flyte
from task import my_task, rust_env

env = flyte.TaskEnvironment(name="hello_trace_py", depends_on=[rust_env])

@env.task
async def pipeline(x: int = 21, label: str = "demo") -> str:
    return (await my_task(x=x, label=label)).upper()
```

```bash
flyte run workflow.py pipeline --x 21 --label demo
```

`depends_on=[rust_env]` is required — it is what carries the Rust task's image
into the deployment plan.

## Building from source

Until the `flyte` crate is published, building needs a sibling checkout of
[flyte-sdk](https://github.com/flyteorg/flyte-sdk) next to this repository (a
path dependency on its `rs_controller` crate):

```
<parent>/
├── flyte-sdk/
└── flyte-sdk-rs/
```

```bash
cargo build && cargo test
```

## Examples

| Example | Shows |
|---|---|
| [`hello-trace`](examples/hello-trace) | Sequential traced steps, structs, and the Python-workflow composition above. |
| [`concurrent-traces`](examples/concurrent-traces) | Many traced steps at once, each recorded and replayed independently. |
| [`retry-replay`](examples/retry-replay) | An expensive step replayed instead of re-run after a failure. |
| [`human-approval`](examples/human-approval) | Pausing for a human decision with `flyte::condition`. |

## Status

v0 supports single-node traces: `#[flyte::task]`, `#[flyte::trace]` with
record/replay, `#[derive(FlyteStruct)]`, and primitives — plus `flyte::condition`
for pausing on an external signal such as a human approval. Not yet: task fan-out
from Rust (a Python parent calling a Rust task works today), a native Rust
launcher, files/dataframes, and trace groups. Expect contract changes while
experimental.

`flyte::condition` needs [flyteorg/flyte-sdk#1401](https://github.com/flyteorg/flyte-sdk/pull/1401)
in the sibling `flyte-sdk` checkout; see [docs/future.md](docs/future.md) for the
design and what is still open.
