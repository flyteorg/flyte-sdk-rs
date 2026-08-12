> [!WARNING]
> **Experimental.** This SDK is a working preview: the APIs, the wire
> contracts, and the internals may all change without notice. Feedback and
> issues are very welcome.

# Flyte Rust SDK

**Write [Flyte](https://github.com/flyteorg/flyte-sdk) tasks in Rust — traced,
replayed on retry, pausable for human approval, and composable with Python
workflows.**

Mark steps of an async fn with `#[flyte::trace]`: each step is recorded as a
child action on the Flyte backend (visible in the console) and **replayed
instead of re-run when the task retries** — in-process checkpointing.
Primitives and serde structs (msgpack, wire-compatible with Python dataclasses)
pass between steps. And with `flyte::condition`, a task can **pause until a
person answers**, then carry on with their answer.

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
lives in [`examples/hello-trace`](examples/hello-trace).

## Pause for a human

`flyte::condition` parks the task until someone signals an answer. Creating a
condition and waiting for it are separate steps, so a task can raise every
question it needs up front — reviewers answer in parallel — and collect the
answers when it is ready to proceed:

```rust
let security = flyte::condition::<bool>("security-review")
    .prompt(format!("Approve **{artifact}** on security grounds?"))
    .markdown()
    .timeout(Duration::from_secs(24 * 60 * 60))
    .create()          // the question exists and is answerable from here
    .await?;

let release = flyte::condition::<String>("release-ticket")
    .prompt(format!("Release ticket for {artifact}?"))
    .create()
    .await?;

// ... other work ...

let (approved, ticket) = futures::try_join!(security.wait(), release.wait())?;
```

The value type is checked at compile time and only `bool`, `i64`, `i32`, `f64`,
`f32` and `String` are allowed, because that is what the backend can validate a
signal against. While the task waits, each condition shows in the console as a
paused action; answer one with:

```bash
flyte get condition <run-name>                              # find the action name
flyte signal condition <run-name> <action-name> true
```

A rejection or a timeout comes back as `Error::Condition`, carrying a
`ConditionOutcome` you can match on. Full example:
[`examples/human-approval`](examples/human-approval).

See [`examples/`](examples) for concurrent traces and replay-on-retry too.

## Install

Two halves. The **crate** compiles into your task container; the **Python
launcher** registers the task with Flyte and builds that container's image.

```bash
pip install flyteplugins-rs
```

```toml
# Cargo.toml
[dependencies]
flyte = "0.1"
```

The two are versioned independently: what keeps them compatible is a descriptor
contract checked at launch, not matching version numbers. See
[docs/releasing.md](docs/releasing.md).

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

## Reusable (warm) containers

By default every action gets its own pod, and pays for scheduling, image pull and
process start before your code runs. [`union-reuse`](https://github.com/unionai/union-reuse)
replaces that with a pool of replicas the backend keeps alive and streams actions
to. It costs one dependency and one changed attribute:

```rust
#[union_reuse::main]   // was #[flyte::main]
#[flyte::task]
async fn warm(x: i64) -> Result<String, flyte::Error> { ... }
```

```python
warm, rust_env = rs.rust_task(
    crate_dir=Path(__file__).parent,
    binary="reusable",
    reuse=flyte.ReusePolicy(replicas=(1, 3), idle_ttl=300, concurrency=4),
)
```

The launcher half — `reuse=` and the image changes it implies — ships here in
[`flyteplugins-rs`](python/flyteplugins-rs); the worker half is the `union-reuse`
crate. A binary built with `#[union_reuse::main]` still runs as an ordinary
one-shot container, so the two can be changed in either order, and dropping
`reuse=` again needs no rebuild.

The image is still declarative layers built by the remote builder — `reuse=` only
adds the second name a pool replica is launched under, so this needs no docker on
your machine. Worked example, including a non-reusable Python parent fanning out
to a reusable Rust child: [`examples/reusable`](examples/reusable).

## Working on the SDK

```bash
cargo build && cargo test
```

The examples import the **released** `flyteplugins-rs` from PyPI, so they read
like a user's project. When changing the launcher itself, install this checkout
over it:

```bash
./scripts/dev-setup.sh          # editable install; undo with
                                # uv pip install --force-reinstall flyteplugins-rs
```

The Rust half needs no switch either. The examples depend on `flyte = "0.1"`
exactly as your crate would, and a `[patch.crates-io]` in the root manifest
redirects that to `crates/flyte` for in-workspace builds — so `cargo build` and
`cargo test` here always exercise the working tree, while each example directory
still builds standalone, which is what the worker image does.

Transport comes from [`flyte_core`](https://crates.io/crates/flyte_core), released
from [flyte-sdk](https://github.com/flyteorg/flyte-sdk) — no sibling checkout
needed. To develop against a local copy, add it to the root `[patch.crates-io]`:

```toml
flyte_core = { path = "../flyte-sdk/rs_controller" }
```

That dependency is temporary: it links `libpython` (via pyo3) and disappears with
the planned pure-Rust controller.

## Examples

| Example | Shows |
|---|---|
| [`hello-trace`](examples/hello-trace) | Sequential traced steps, structs, and the Python-workflow composition above. |
| [`concurrent-traces`](examples/concurrent-traces) | Many traced steps at once, each recorded and replayed independently. |
| [`retry-replay`](examples/retry-replay) | An expensive step replayed instead of re-run after a failure. |
| [`human-approval`](examples/human-approval) | Pausing for a human decision with `flyte::condition`. |
| [`reusable`](examples/reusable) | A warm container: one pool of replicas serving many actions, with a non-reusable Python parent fanning out to it. |

## Status

v0 supports single-node traces: `#[flyte::task]`, `#[flyte::trace]` with
record/replay, `#[derive(FlyteStruct)]`, and primitives — plus `flyte::condition`
for pausing on an external signal such as a human approval. Not yet: task fan-out
from Rust (a Python parent calling a Rust task works today), a native Rust
launcher, files/dataframes, and trace groups. Expect contract changes while
experimental.

`flyte::condition` has been run end to end against a live backend — task pauses,
`flyte signal condition` resolves it, task resumes with the value. It needs
[flyteorg/flyte-sdk#1401](https://github.com/flyteorg/flyte-sdk/pull/1401),
which is merged and is the `flyte_core` rev pinned here.

**Packaging.** Both halves are released: the crate as
[`flyte`](https://crates.io/crates/flyte) on crates.io, the launcher as
[`flyteplugins-rs`](https://pypi.org/project/flyteplugins-rs/) on PyPI. Reusable
containers are a separate crate in a separate repo,
[`union-reuse`](https://github.com/unionai/union-reuse).

The worker still embeds a Python interpreter — the transport crate
[`flyte_core`](https://crates.io/crates/flyte_core) enables `pyo3/auto-initialize`
by default, so task binaries link `libpython` and the image installs
`python3-dev`. That ends with the pure-Rust controller swap.
