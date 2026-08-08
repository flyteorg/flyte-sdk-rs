# Designed, not yet built

Things this SDK is expected to grow, written down with enough detail to be picked up. Nothing
here is implemented; nothing here has run against a cluster.

---

# `flyte::condition` — wait for an external signal

A task pauses until something outside it provides a value: a human approving a deploy, an
external system calling back. The Python SDK's primitive is
`flyte.new_condition(name, ...)` + `condition.wait()`; the CLI calls the same thing a *signal*.

**Why `condition` and not `signal`:** it matches the Python SDK, and `tokio::signal` is the
module for Unix process signals — `flyte::signal` would read as `SIGTERM` handling to a Rust
developer. Rust's own blocking primitive is `Condvar`, so `Condition` is familiar and unclaimed.

## How it works on the wire

A condition is **not a pod**. It is a child action inserted directly in phase
`ACTION_PHASE_PAUSED`; it never reaches the leasor and never gets scheduled. The **parent task
pod stays alive** and waits on the `WatchForUpdates` stream the informer already holds. When
someone signals it, the value arrives **inline** on `ActionUpdate.value` as a `core::Literal` —
no blob storage round trip.

Everything needed is already generated in `flyteidl2 = 2.0.37`, the crate this SDK already
depends on:

| Piece | Location |
|---|---|
| `ConditionAction` | `flyteidl2.workflow.rs:78` |
| `Action.spec::Condition` | `flyteidl2.actions.rs:39-51` |
| `ActionUpdate.value` (tag 5) | `flyteidl2.workflow.rs:854` |
| `ActionPhase::Paused` (9) | `flyteidl2.common.rs:587` |
| `ActionsService::signal` | `flyteidl2.actions.tonic.rs:186` |
| `RunService::signal_event` | `flyteidl2.workflow.tonic.rs:1908` |

No proto regeneration and no backend work are required.

Two properties of the existing code make the SDK side small:

- **`submit_action` is already the wait.** `CoreBaseController::submit_action` enqueues and then
  blocks until it sees a terminal `ActionUpdate`, and `Paused` is correctly *not* terminal
  (`rs_controller/src/action.rs:139`). That is precisely a condition's lifecycle, so
  `submit_action` needs no change.
- **Action naming is already compatible.** Python derives a condition's action name with the
  same `md5(parent-input_hash-identity-seq) → base36` scheme this SDK implements, passing the
  condition's *name* as both `input_hash` and `identity`
  (`flyte-sdk/src/flyte/_internal/controllers/remote/_controller.py:705-708`). So
  `crates/flyte/src/hash.rs::sub_action_name` and `crates/flyte/src/context.rs::Sequencer` are
  reusable verbatim.

## Proposed API

A builder, with the value type inferred from the binding rather than passed as an argument the
way Python's `data_type` is:

```rust
let approved: bool = flyte::condition("approve-deploy")
    .prompt("Ship build 1234 to production?")
    .markdown()
    .description("Requires a release manager")
    .timeout(Duration::from_secs(3600))
    .wait()
    .await?;
```

```rust
pub fn condition(name: impl Into<String>) -> ConditionBuilder;

impl ConditionBuilder {
    pub fn prompt(self, prompt: impl Into<String>) -> Self;
    pub fn markdown(self) -> Self;                       // prompt_type = Markdown
    pub fn description(self, description: impl Into<String>) -> Self;
    pub fn timeout(self, timeout: Duration) -> Self;
    pub fn webhook(self, url: impl Into<String>) -> Self;
    pub async fn wait<T: ConditionValue>(self) -> Result<T, Error>;
}
```

Type inference comes from the binding, so the common case needs no annotation beyond the `let`.
Turbofish (`.wait::<bool>()`) is only needed where the type is otherwise unconstrained — e.g.
`if flyte::condition("ok").wait::<bool>().await? { .. }`.

### Only four types, enforced at compile time

The backend accepts only `bool`, integer, float, and string values, so a
`#[derive(FlyteStruct)]` type must be a **compile** error rather than a runtime rejection:

```rust
mod sealed { pub trait Sealed {} }

/// Values a condition can carry. Sealed: the backend accepts only simple types.
pub trait ConditionValue: sealed::Sealed + FlyteType {}
```

implemented for `bool`, `i64`, `i32`, `f64`, `f32`, `String` only. Decoding the delivered
literal reuses the existing `FlyteType::from_literal` — there is no second decoder.

### Errors

One new `Error` variant carrying an outcome enum, rather than three flat variants or a bare
`User` error:

```rust
pub enum ConditionOutcome { TimedOut, Failed, Aborted }

// Error::Condition { name: String, outcome: ConditionOutcome, message: String }
```

The reason is `crates/flyte/src/worker.rs::error_document`: its catch-all maps every non-`User`
error to `("SystemError", System)`. A timed-out *approval* written into `error.pb` as a system
error would carry the wrong kind and the wrong retry semantics, and would diverge from Python
(which raises `ConditionTimedoutError`, a user error). `ConditionOutcome::code()` restores
Python's codes while keeping the interesting axis exhaustively matchable by user code.

Open question: whether to mark `Error` `#[non_exhaustive]` at the same time. Adding a variant is
a breaking change for anyone matching exhaustively; doing it now, while the SDK is experimental,
is cheaper than later.

### Laziness and cancellation

- **Rust futures are lazy**, so `flyte::condition(..)` alone registers nothing — `.wait()` is
  what enqueues the action. This avoids a Python footgun where registering and never awaiting
  leaves a `PAUSED` action behind forever.
- **Dropping the future mid-wait leaves a `PAUSED` action.** A `tokio::select!` loser abandons
  the wait locally, and the SDK **cannot** abort the action: `cancel_action`
  (`rs_controller/src/core.rs:574`) only marks the local cache and fires a local completion
  event — it never calls `ActionsService/Abort`. Until that gap is closed, a server-side
  `.timeout(..)` is the only thing that reaps an abandoned condition. Always set one when
  racing a condition against anything else.

### Composition

```rust
// Race an approval against a deadline. Note the .timeout() as well — see above.
tokio::select! {
    approved = flyte::condition("approve").timeout(Duration::from_secs(600)).wait() => approved?,
    _ = tokio::time::sleep(Duration::from_secs(300)) => false,   // leaves the condition PAUSED
}

// Several independent approvals at once.
let (security, legal) = futures::try_join!(
    flyte::condition("security-review").wait::<bool>(),
    flyte::condition("legal-review").wait::<bool>(),
)?;
```

The naming rule mirrors traces — distinct names get distinct counters, so names (and therefore
resumption after a retry) do not depend on completion order.

**But the analogy stops there.** For traces, two calls that share a counter are byte-identical
and therefore interchangeable. Two conditions sharing a name are **not**: they are two
questions a human answers separately, possibly with different values, behind an identical
prompt. Reusing a condition name within one task is legal (each gets its own `seq`) but almost
always a mistake — give each decision its own name.

### Local mode

With no runtime state installed (`context::current()` is `None`, i.e. plain `flyte::run` in a
test), `.wait()` returns an error immediately rather than hanging — mirroring Python's
`RuntimeError("Conditions can only be awaited within a task context.")`. A future test hook
(inject a value by condition name) would let approval flows be unit-tested; not designed yet.

### Wire details to match Python exactly

The non-obvious part. All verified against the Python SDK and the backend.

| Field | Value |
|---|---|
| `inputs_uri` | `{run_base_dir}/{condition_action_name}/inputs.pb` — **nothing is uploaded there**. Must be non-empty for two independent reasons: the server's enqueue validator, and `rs_controller/src/core.rs:632-653` `build_action_scalars`, which fails client-side before any RPC when it is `None`. |
| `run_output_base` | set; also required by `build_action_scalars`. |
| `ConditionAction.type` | `LiteralType{simple: ..}` from the value type. Effectively required — `flyte-sdk/src/flyte/cli/_signal.py:48-51` refuses to signal a condition whose type the backend never received. |
| `prompt` / `description` | default `"Approve?"` / `""`. |
| `prompt_type` | `Text = 1`, `Markdown = 2` (`flyteidl2.workflow.rs:641`; `Unspecified = 0`). |
| `timeout` | `google.protobuf.Duration`, set **only** when strictly positive — the proto documents that zero or negative is ignored. |
| headers | the four `x-actions-project` / `-domain` / `-run` / `-parent-action`. `rs_controller::actions_metadata` already emits exactly these; no work needed. |
| local `phase` | `Unspecified` — **not** `Succeeded`. Traces set a terminal phase because they are *recorded* after the fact; a condition is *launched* and resolved by the server. |
| `realized_outputs_uri` | never set — the value arrives inline on `ActionUpdate.value`. |
| parent | the **task** action, not an enclosing trace. In this SDK that is automatic: `RuntimeState::action_name` is the task action. |
| `ACTION_PHASE_RECOVERED` | success-equivalent. Map phases with an explicit `_ =>` catch-all, never `Some(Succeeded)` alone. |

### What has to change in `rs_controller`

This is the blocker, and it lives in a **different repository** (`../flyte-sdk`). All six edits
are additive; none alters existing task or trace behavior.

1. `src/action.rs:15` — add `ActionType::Condition = 2` (currently `Task | Trace` only).
2. `src/action.rs:22` — add `condition: Option<ConditionAction>` and
   `condition_output: Option<core::Literal>` to `Action`; add an `Action::from_condition(..)`
   constructor.
3. `src/action.rs:100` `merge_update` — propagate `obj.value` into `condition_output`. This is
   the one line that currently discards the signalled payload: the method reads `obj.phase`,
   `obj.error`, and `obj.output_uri`, but never `obj.value`.
   Do it **un-gated**, unlike Python, which gates on `self.type == "condition"`. An entry
   created by `new_from_update` has `action_type: Task` and nothing later repairs it, so a
   gated read would silently drop the value of an already-signalled condition on retry.
4. `src/action.rs:115` `new_from_update` — same propagation, and stop hardcoding
   `action_type: ActionType::Task`.
5. `src/core.rs:686` and `:708` — `Condition` arms in `build_queue_spec` / `build_actions_spec`.
   Both proto oneofs already have the variant.
6. `src/core.rs:796` `launch_task` — `ActionType::Condition => action.condition.is_some()` in
   the spec-presence check.

Explicitly **not** required: any change to `submit_action`, any header work, and any new RPC
wrapper — a `signal` wrapper would only be needed to *send* signals, which is out of scope
below.

### Where the SDK code would go

- `crates/flyte/src/condition.rs` (new) — the builder and the protocol, mirroring `trace.rs` in
  shape and doc style.
- `crates/flyte/src/idl.rs` — re-export `ConditionAction`, `ConditionPromptType`; the only place
  proto types enter the SDK.
- `crates/flyte/src/controller.rs` — a `submit_condition`-style method speaking **only**
  SDK-owned types, preserving the swap boundary.
- `crates/flyte/src/lib.rs` — export `condition`, `ConditionValue`, `ConditionOutcome`.

### Tests and example

- **Name-derivation golden**, alongside the existing `sub_action_name_matches_python`: for
  parent `a0`, condition name `approve`, `seq = 1`, the action name is
  `9y1pejgwkesmjwxx7iy22lor3` (`seq = 2` gives `2yd16vru0yb9hcfvi6tbus5pd`). Both were computed
  with a generator independently checked against the already-pinned
  `ape1kkafckt4ekjb0537lcq3u`.
- **Type restriction**: `compile_fail` doctests, preferred over `trybuild` because there is no
  pinned stderr to churn on toolchain bumps — paired with a `no_run` positive case so the
  negative tests cannot rot into vacuous truth.
- **Error mapping**: each terminal phase → the expected `ConditionOutcome` and code.
- **Example** `examples/human-approval/` in the established layout (`src/main.rs` + `task.py` +
  `interface_gen.py` via `python/flyteplugins-rs`), gating an action behind an approval.

End-to-end verification, once the `rs_controller` edits land: run the example, confirm the
console shows a `PAUSED` child action while the task waits, then satisfy it with

```bash
flyte signal condition <run-name> <action-name> true
```

and confirm the task resumes and returns. Note the usability wrinkle worth documenting for
users: `flyte signal condition` takes the **hashed action name**, not the friendly condition
name — `flyte get condition <run-name>` is how you find it.

### Out of scope for v1

**Sending** signals from Rust (so one Rust task could unblock another). That needs a
`RunService::signal_event` wrapper plus the `EventPayload` encoding, which is a second codec
alongside the `Literal` one. Humans and Python can already signal, which covers the
human-in-the-loop case completely.

---

# Other known gaps

Each of these was established with evidence; recorded so it need not be re-derived.

**Reusable containers (fasttask).** Python tasks running in reusable containers report
completion over a live channel, with no pod lifecycle — noticeably faster than a pod-per-action
task. The Rust worker has no fasttask protocol support, so every Rust action pays pod startup
and teardown.

**Native Rust launcher (`CreateRun`).** Today launching requires the Python SDK as a
launch-time client. The `describe-interface` descriptor is the first piece of removing that: it
already gives a launcher everything it needs about a task's inputs and outputs, in declaration
order.

**Two upstream image-IDL fixes would make Rust images first-class in the remote builder.**
There is no `Entrypoint` layer in the imagebuilder `Layer` oneof, so a DSL-built image has no
`ENTRYPOINT` and the task compensates by putting the binary path in `args[0]`. And
`ImageSpec.platform` exists in the proto but is never set by the Python SDK nor parsed by the Go
frontend, so `Image.platform` is silently ignored for remote builds — you get the builder pod's
architecture.

**Single-stage builds, no cargo caching.** The image DSL has no notion of build stages, so a
DSL-built worker image carries the Rust toolchain and sources (roughly ten times the size of the
hand-written multi-stage image in `docker/Dockerfile`). Worse for iteration: the image tag is a
content hash of the copied sources, so any Rust change triggers a full uncached
`cargo build --release` in the builder. Multi-stage support plus a cached cargo registry layer
would fix both.

**`cancel_action` does not abort server-side.** `rs_controller/src/core.rs:574` marks the local
cache and fires a local completion event, but never calls `ActionsService/Abort`. Pre-existing,
and mostly invisible today — conditions would make it user-facing, since an abandoned wait
leaves a `PAUSED` action that only a server-side timeout will reap.
