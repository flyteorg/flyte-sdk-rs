# flyte-sdk-rs

Rust SDK for Flyte v2 — **v0 (PoC): single-node traces**.

Write a task as an async fn, mark steps with `#[flyte::trace]`, and run the
binary as a Flyte task container. Traced steps execute in-process, are recorded
as child trace actions on the backend (visible in the console), and are
**replayed on retry** (checkpoint/resume). Primitives (`i64`/`i32`/`f64`/`f32`/
`String`/`bool`) and serde structs (`#[derive(FlyteStruct)]`, msgpack-encoded,
wire-compatible with Python dataclasses) pass between steps.

```rust
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, flyte::FlyteStruct)]
struct Stats { mean: f64, count: i64, label: String }

#[flyte::trace]
async fn compute_stats(total: i64, label: String) -> Result<Stats, flyte::Error> { ... }

#[flyte::main]                                        // generates fn main()
#[flyte::task]
async fn my_task(x: i64, label: String) -> Result<String, flyte::Error> {
    let stats = compute_stats(x * 2, label).await?;   // recorded + replayed on retry
    ...
}
```

That is the whole file — `#[flyte::main]` emits
`fn main() { flyte::worker_main(my_task_entry()) }` beside the task, so nothing
in user code names the worker plumbing. Pair it with `#[flyte::task]` (which
generates the `_entry` fn it calls) on a fn at the crate root of a bin target;
attribute order doesn't matter.

## Demo: run it locally (no backend)

Traced fns simply run their bodies when no backend is attached, so `flyte::run`
executes the whole task in-process:

```rust
#[test]
fn runs_locally() {
    let out = flyte::run(my_task(21, "demo".to_string())).unwrap();
    assert_eq!(out, "demo: mean=21 over 2 values");
}
```

```
cargo test -p hello-trace
```

## Smoke test: record + replay against a real control plane

`scripts/smoke.sh` runs the example worker twice with the same run identity:

- **attempt 1** (`FLYTE_ATTEMPT_NUMBER=0`): executes all three traced steps and
  records each as a child trace action via the ActionsService;
- **attempt 2** (`FLYTE_ATTEMPT_NUMBER=1`): must **replay all three traces**
  from the backend (the deterministic action names match across processes, the
  informer's watch stream delivers the recorded actions, and the bodies are
  skipped).

It fails loudly if attempt 1 replays anything or attempt 2 replays fewer than 3.

### Against a hosted cluster (e.g. demo.hosted.unionai.cloud)

```bash
# one-time: create client credentials (prints an export FLYTE_API_KEY=... line)
cd ../flyte-sdk && uv run flyte --config ~/.flyte/demo-config.yaml \
    create api-key --name rust-sdk-smoke

FLYTE_API_KEY="<that key>" ./scripts/smoke.sh
# ...
# replayed traces: 3 (expected 3)
# SMOKE PASSED: mode=api-key run=rust-smoke-<ts> org=demo project=flytesnacks domain=development
```

Defaults: `org=demo project=flytesnacks domain=development`; override with
`SMOKE_ORG` / `SMOKE_PROJECT` / `SMOKE_DOMAIN`. Copy the API key carefully —
it is url-safe base64 and the CLI wraps it across lines.

### Against the local devbox

Start the devbox control plane (bundles all services in one process, listens on
`localhost:8090`, no auth):

```bash
cd ../cloud/devbox && make devbox-start   # needs the k3d devbox cluster + localstack
```

Then, with no `FLYTE_API_KEY` set, the script targets the devbox:

```bash
./scripts/smoke.sh
# defaults: org=testorg project=testproject domain=development, endpoint localhost:8090
```

Override the endpoint with `SMOKE_ENDPOINT=host:port` if yours differs.

### What the smoke test proves

Client-credentials auth + TLS (hosted mode), ActionsService enqueue of trace
actions (`_U_USE_ACTIONS=1` — the legacy Queue/State path is not supported),
the watch-stream informer sync, deterministic sub-action naming across
processes, and end-to-end replay of primitives and msgpack structs. The run
identity is synthetic (the worker runs on your host, standing in for a task
pod), so it won't render as a full run in the console; running as a real
in-cluster task container is the same code path with the backend supplying the
args/env.

## Demo: run as a real task container on a cluster

```bash
cargo build -p hello-trace     # once: the launcher reads the interface from the binary

cd examples/hello-trace
uv run --project ../../../flyte-sdk flyte run \
    --project <project> --domain <domain> rust_task.py my_task --x 21 --label demo
# url: https://<cluster>/v2/domain/<domain>/project/<project>/runs/<run-id>
# o0: "demo: mean=21 over 2 values"
```

Nothing to configure: the worker image is declared as `flyte.Image` layers
(`rust_worker_image()` in `examples/hello-trace/rust_task.py`) and built by
Flyte's image builder on the first run — no Dockerfile, no docker on your
machine, no registry push. This works with the **remote** builder (`image:
{builder: remote}` in config): the backend only creates a Python venv when a
Python layer asks for one, so the Rust-only layer stack (apt + copy +
`cargo build`) produces an image with no Python environment. Later runs reuse
the built image.

The console then shows the run with the root action `a0` plus one child trace
action per `#[flyte::trace]` step (`double`, `compute_stats`, `describe`),
all recorded by the Rust SDK from inside the container.

Once the `flyte` crate is published to crates.io, the build context shrinks to
**just the user's crate folder** — cargo fetches the SDK like any other
dependency. The extra `with_source_folder` calls shipping the workspace and
`rs_controller` exist only for today's path dependencies and are marked for
deletion.

Notes:

- `flyte run` always uploads a small Python code bundle the Rust container never
  reads (the CLI has no `--version` flag and `--copy-style none` requires one);
  programmatic launches can pass `copy_style="none"`. `--local` is unsupported —
  local mode would have to run the container itself.
- Image-IDL limits, both fixable upstream: no ENTRYPOINT layer exists (the task
  compensates by putting the binary path in `args[0]`), and the remote builder
  ignores `Image.platform` (you get the buildkit pod's arch).
- Prefer a small hand-optimized image? `scripts/build-image.sh` builds a
  multi-stage one (~147MB vs ~1.5GB single-stage) to push yourself; swap the
  task's image for `flyte.Image.from_base("<pushed-uri>")`.

## The task interface is generated, not declared

The binary prints its own interface, derived by `#[flyte::task]` from the fn
signature:

```bash
cargo run -p hello-trace -- describe-interface
# {"flyte_interface_version":1,"task":"my_task","inputs":[{"name":"x","type":"integer","required":true},
#  {"name":"label","type":"string","required":true}],"outputs":[{"name":"o0","type":"string"}]}
```

`rust_task.py` builds its `NativeInterface` from exactly that. On a dev machine
it reads the built binary live and refreshes the generated `interface_gen.py`
when stale; inside a task container (the workflow path imports the module in
the parent's container, where there is no cargo build) the generated module
stands in. So the Rust signature is the only place the interface is written
down — rename a parameter and the launcher follows instead of the container
failing with `inputs missing <name>`. This descriptor is also the first piece
of the client-side launcher.

## Use it from a Python workflow

A Python parent can call the Rust task as a child action, with **no Rust-side
changes** — the parent writes the child's `inputs.pb` and reads back its
`outputs.pb`, which is what the worker already does:

```python
from rust_task import my_task, rust_env

env = flyte.TaskEnvironment(name="hello_trace_py", depends_on=[rust_env])

@env.task
async def pipeline(x: int = 21, label: str = "demo") -> str:
    return (await my_task(x=x, label=label)).upper()
```

`depends_on=[rust_env]` is required, not decorative: it puts the Rust image in the
deployment plan and therefore in the image cache handed to the child at submit
time. It is also sufficient — the child's task spec is built inline at enqueue, so
the Rust environment needs no separate `flyte deploy`. See
`examples/hello-trace/workflow.py`.

## Worker container contract

The worker honors the Python SDK's container contract, so the backend can
launch it like any task:

- **args**: `a0 --inputs <uri> --outputs-path <uri> --name {{.actionName}}
  --run-name {{.runName}}` (unknown/unused flags are tolerated). Only
  `{{.input}}` and `{{.outputPrefix}}` are substituted by the backend —
  `{{.runName}}` and `{{.actionName}}` are **not** in its substitution set, so the
  worker discards any `{{...}}` value and takes those names from env instead.
  `--run-base-dir` is never passed as an arg; it arrives only as `_U_RUN_BASE`.
- **env**: `ACTION_NAME`, `RUN_NAME`, `FLYTE_INTERNAL_EXECUTION_PROJECT`,
  `FLYTE_INTERNAL_EXECUTION_DOMAIN`, `_U_ORG_NAME`, `_U_RUN_BASE`,
  `_UNION_EAGER_API_KEY` (or `EAGER_API_KEY`), `_U_EP_OVERRIDE`, `_U_INSECURE`,
  `FLYTE_ATTEMPT_NUMBER`.
- **behavior**: loads `inputs.pb`, runs the task fn (traces record/replay),
  uploads `outputs.pb` on success or `error.pb` on failure, always exits 0.
- storage: `s3://` (honors `AWS_ENDPOINT_URL` for minio/localstack), `gs://`,
  `az://`/`abfs(s)://`, `file://`/bare paths.

## Architecture

- Reuses `../flyte-sdk/rs_controller` (`flyte_core`) as a path dependency for
  transport: unified **ActionsService** enqueue + watch, informer cache,
  client-credentials auth from `_UNION_EAGER_API_KEY`.
- Implements natively (what the Python layer does): literal conversion, blob IO
  (`inputs.pb`/`outputs.pb`/`error.pb` via `object_store`), deterministic
  sub-action names (`md5→base36`, byte-compatible with Python), the trace
  replay/record protocol, and the worker entrypoint.
- **Swap boundary**: rs_controller/pyo3 touchpoints live only in
  `crates/flyte/src/controller.rs`; proto types are only imported via
  `crates/flyte/src/idl.rs`; macros emit only `::flyte::` paths. Replacing the
  pyo3-linked controller with a pure-Rust/tonic one later touches those two
  files and no user code.

## Building

`cargo build` links `libpython3.10+` (transitively via pyo3 in `flyteidl2` /
`flyte_core` — temporary, see swap boundary above). If discovery fails:
`PYO3_PYTHON=$(which python3.12) cargo build`.

No Python *runs* in the task container: the image's `ENTRYPOINT` is the Rust
binary, the backend leaves the container `command` empty, and nothing on the SDK's
path acquires the GIL — the interpreter is linked but never initialized. libpython
must still be present for the dynamic loader, which is why `docker/Dockerfile`
installs `libpython3.11`. The swap to a pure-Rust controller removes that too.

## Tests

`cargo test` — includes cross-language golden tests (sub-action naming, inputs
hash, mashumaro msgpack bytes) generated from the Python SDK; the generating
snippets are in comments in `crates/flyte/tests/golden.rs`.

## Not yet in v0

Task fan-out from Rust (child task actions — a *Python* parent calling a Rust task
already works, see above), client-side launcher (`CreateRun`; the interface
descriptor is its first piece), files/dataframes, trace groups, error replay for
failed traces (they re-run instead).
