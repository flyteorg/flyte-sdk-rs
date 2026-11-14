# Calling tasks from Rust tasks

Status: Track 1 implemented and run end to end (a Rust parent fanning out to 19 Rust children, demo run ulj7rsm55n8mp92vpqxm); Track 2 designed, not built.

## The contract that does not change

```rust
static COMPILE: flyte::TaskRef = flyte::TaskRef::new("ci_env.compile");

let out: String = flyte::call_task(&COMPILE, flyte::inputs! { pkg: &pkg, sha: &sha }).await?;
let out: String = flyte::call_task(&COMPILE.with_retries(3), flyte::inputs! { .. }).await?;
```

A call site names a task. It never says where the task's `TaskSpec` comes
from. That is a `SpecSource` (`#[non_exhaustive]`), resolved when the call is
made. Each track below only adds a source or a resolution step, so no call site
is ever rewritten. The pieces:

| piece | role | stable? |
|---|---|---|
| `TaskRef::new(name)` (const), `TaskRef::named(String)` | the call-site handle | yes |
| `TaskRef::from_spec_bytes`, `TaskRef::from_source` | explicit sources | yes; new constructors are additive |
| `SpecSource::{Named, Inline}` | where a spec comes from | additive (non_exhaustive) |
| `with_retries`, `with_short_name` | Python's `override()`; identity unchanged | more overrides are additive |
| `call_task::<O>` with `O: TaskOutput` (`()`, any `FlyteType` as `o0`, `TaskOutputs`) | the call | yes |
| `inputs!` / `TaskInputs` | named inputs, put in the task's order | yes |
| `stub_task` | in-process stand-in when there is no backend | yes |

## What every source must provide

Whatever produces it, a call needs a `ResolvedSpec`:

1. **the `TaskSpec`**, submitted as is, apart from per-call overrides;
2. **the declared input order**. Python's inputs hash concatenates inputs in
   `native_interface` order, which is the order the function declares them. The
   proto interface keeps the names sorted. So `_submit` (calling a task object)
   and `_submit_task_ref` (calling a fetched reference) hash the same call
   differently whenever the arguments are not alphabetical. A source that knows
   the declared order supplies it and matches `_submit`. One that does not falls
   back to interface order and matches `_submit_task_ref`. Either way the result
   is exactly what Python does for the same kind of reference.
3. **the interface hash and task identity**, computed once per spec. The
   interface hash comes from the producer's own serialized bytes. Python's
   deterministic serialization sorts map keys, and prost's `HashMap` encoding
   does not, so re-encoding would diverge for interfaces with
   `TypeStructure.dataclass_type` maps.

The action name, call sequence and cache key are derived from these exactly as
in `RemoteController._submit`. That is pinned by `call_names_match_python`,
whose golden values the Python SDK generates (`scripts/gen_call_task_golden.py`).

## Track 1: specs bundled by the launcher (done)

`rust_task(calls=[a, b])` in flyteplugins-rs serializes each callee at deploy
time with Python's own `translate_task_to_wire`, under the same context
`_submit` would build at call time: the deploy's version and image cache, and
no input/output paths. The callees and their declared input orders go into the
parent's container args as `--task-specs <base64 JSON>`.

- `flyte::worker::parse_args` registers the bundle as it parses. That covers
  the one-shot worker and union-reuse, which parses each assignment's command
  with it. `_F_RS_TASK_SPECS` is an env fallback.
- Nesting is transitive: a callee's spec carries its own `--task-specs`.
- Callee environments are added to `depends_on`, so their images are built.
  Callees need no separate deployment.
- At call time the spec's `id.org/project/domain` are set to the calling run's,
  as Python's `_submit` builds a callee under the current action's. Deploy-time
  serialization leaves them empty, and the backend rejects that. None of these
  fields is hashed.
- Specs are frozen at the parent's deploy. That is the same as Python, where a
  parent always launches the callee code it was deployed with.

## Track 2: specs from anywhere

### (a) Reference tasks: fetch a deployed task

New variant `SpecSource::Remote { name, version: VersionSel }`, where `VersionSel`
is `Exact(String) | Current | Latest`. `Named` never falls back to it: a
missing `calls=` entry is an error, not a silent fetch of whatever is deployed.
Fetching is always asked for, with an explicit `TaskRef::remote`.

- Fetch through `flyteidl2` `TaskService.GetTaskDetails` (or `ListTasks` for
  `Latest`) on the controller's authenticated channel. `flyte_core`'s
  `bin/try_list_tasks.rs` shows the channel setup. The client belongs behind
  `controller.rs`, the existing swap boundary.
- Cache per process by `(name, resolved version)`. Concurrent first calls share
  one fetch through a `OnceCell` per key.
- `Current` means the parent's own version. That needs the worker to read
  `--version`, which it accepts and drops today.
- Input order: `TaskDetails` has no declared order, so hash in interface order.
  That matches `_submit_task_ref`, which is what Python does for the same
  reference.
- Callees must be deployed first. That is a deploy-ordering problem for the
  launcher, not the SDK.

### (b) Native Rust task definitions

`#[flyte::task]` gains enough metadata (cache, retries, resources, image ref)
for Rust to build a `TaskSpec` without Python. The new variant is
`SpecSource::Native(&'static TaskEntry)`, so a `TaskRef` can come from a Rust fn
item. The hard part is the image:

- The worker must parse `--image-cache` (base64, gzip and JSON of
  `ImageCache`; Python's `flyte/_internal/imagebuild/image_builder.py`) and
  resolve an environment's image the way `lookup_image_in_cache` does.
- The container args contract (`rust_task.container_args`) moves into Rust.
- The `discovery_version` source must match the Python launcher's for the same
  binary. Otherwise native and bundled specs of one task get different
  identities, and recovery across the switch misses. Simplest rule: take it from
  the descriptor, so both sides read one value.

This is the README's "native Rust launcher". It is a superset of (a) in effort,
and it is what removes Python from the deploy path.

### Override semantics

Overrides must never change identity, because identity is what recovery
matches on. Today: `retries` sets `metadata.retries`, and `short_name` sets
`TaskSpec.short_name`. Next, in Python's `override()` order: `resources`,
`env_vars`, `timeout`, `cache` (changes the key, not the identity), and
`max_inline_io_bytes`. Each is a field on `Overrides`, applied to the
`TaskSpec` clone in `launch`.

## flyte_core is vendored, patched

`crates/flyte` depends on `vendor/flyte_core` by path: crates.io 0.1.0 plus
fixes that flyte-sdk has not released (`vendor/flyte_core/PATCHES.md`). Without them, the first demo fan-out run
(ugq9mw6f6zjr99pss8w6) hung. All 19 children succeeded, and the parent never
heard. 0.1.0 spins forever on a watch stream the server ended, and it never
reconnects. Its channel also sends no keepalive pings, so a silently dropped
connection hangs the same way. The first fix is backported from flyte-sdk main,
which has not been released. The second is in neither. A git or path dependency
on `flyte` picks up the vendored crate, so consumers need no extra
`[patch]`. The path dependency has no `version`, so `cargo publish` fails
until it is replaced with a flyte_core release carrying the fixes, and
`tests/watch_reconnect.rs` must still pass against that release.

## Known gaps

- **Cancel aborts only through ActionsService.** Over StateService (without
  `_U_USE_ACTIONS=1`) a cancelled child runs on, as in flyte-sdk main.
- No `Vec`/`Map`/`File`/`Dir` `FlyteType`s. Collections travel as JSON strings
  for now.
- One output (`o0`) is typed. Tuples for multiple outputs are additive on
  `TaskOutput`.
- `custom_context` is not forwarded into child inputs. Python adds it to
  `Inputs.context`, which is outside the hash.

## Open questions

1. Upstreaming: the `--task-specs` arg is a flyteplugins-rs ↔ flyte contract,
   versioned as `{"v": 1, "tasks": [...]}`. The Rust side rejects any version
   it does not know. Is that the right compatibility rule once both sides
   release separately?
