# Designed, not yet built

Things this SDK is expected to grow, written down with enough detail to be picked up. Unless a
section says otherwise, nothing here is implemented and nothing here has run against a cluster.

---

# `flyte::condition` — wait for an external signal

> **Implemented.** The SDK side is in `crates/flyte/src/condition.rs` with an example in
> `examples/human-approval`, and the crate side is
> [flyteorg/flyte-sdk#1401](https://github.com/flyteorg/flyte-sdk/pull/1401). Until that PR
> merges, this needs the sibling `flyte-sdk` checkout on that branch. Not yet exercised against
> a live cluster — the one thing left to verify is the round trip in the "Tests and example"
> section below.

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

One property of the existing code makes the SDK side smaller than it looks:

**Action naming is already compatible.** Python derives a condition's action name with the same
`md5(parent-input_hash-identity-seq) → base36` scheme this SDK implements, passing the
condition's *name* as both `input_hash` and `identity`
(`flyte-sdk/src/flyte/_internal/controllers/remote/_controller.py:705-708`). So
`crates/flyte/src/hash.rs::sub_action_name` and `crates/flyte/src/context.rs::Sequencer` are
reusable verbatim, and the names a Rust task produces match the ones Python would.

`Paused` is also correctly excluded from `is_action_terminal`, so a waiter keeps waiting through
it without any special casing.

## Proposed API

**Creating a condition and waiting for it are separate steps**, as they are in Python. That is
deliberate and load-bearing: creating it is what makes the condition exist — it appears in the
console for a human to answer, and its webhook fires — so a task can raise several questions up
front, get on with other work, and collect the answers later. Collapsing the two into one call
would mean nothing is ever visible until the task is already blocked on it.

```rust
// Ask now: the condition action is created here and is immediately visible.
let approval = flyte::condition("approve-deploy")
    .prompt("Ship build 1234 to production?")
    .markdown()
    .description("Requires a release manager")
    .timeout(Duration::from_secs(3600))
    .create()
    .await?;

// ... do other work, or hand `approval` to whatever owns the decision ...

// Collect the answer, here or somewhere else entirely.
let approved: bool = approval.wait().await?;
```

```rust
pub fn condition<T: ConditionValue>(name: impl Into<String>) -> ConditionBuilder<T>;

impl<T: ConditionValue> ConditionBuilder<T> {
    pub fn prompt(self, prompt: impl Into<String>) -> Self;
    pub fn markdown(self) -> Self;                       // prompt_type = Markdown
    pub fn description(self, description: impl Into<String>) -> Self;
    pub fn timeout(self, timeout: Duration) -> Self;
    pub fn webhook(self, url: impl Into<String>) -> Self;
    /// Register the condition. It exists, and is answerable, from here on.
    pub async fn create(self) -> Result<Condition<T>, Error>;
}

pub struct Condition<T: ConditionValue> { /* action name + declared type */ }

impl<T: ConditionValue> Condition<T> {
    /// Block until it is signalled (or fails, times out, or is aborted).
    pub async fn wait(&self) -> Result<T, Error>;
    /// The action name, which is what `flyte signal condition` takes.
    pub fn action_name(&self) -> &str;
}
```

The value type is pinned at `create()` rather than at `wait()`, because the declared
`LiteralType` is part of the registration payload — the backend needs it to validate a signal,
and the CLI needs it to format one. Inference still works from a later `wait()` binding
(`let ok: bool = approval.wait().await?`), so the turbofish is usually unnecessary.

Because `Condition<T>` is a plain handle, the two halves can live far apart: pass it between
functions, park several in a `Vec`, or `futures::try_join_all` over their `wait()`s.

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

### Lifecycle and cancellation

- **`create()` enqueues; `wait()` only waits.** The condition exists from `create()` onward,
  independently of whether anyone is waiting yet. This is what the decoupling buys, and it is
  why `create()` is `async` while the builder before it is not.
- **A created-but-never-awaited condition stays `PAUSED`.** That is inherent to the decoupling
  (Python has the same property) and is the reason `.timeout(..)` matters: it is the only thing
  that reaps one, since the SDK cannot abort a server-side action — `cancel_action`
  (`rs_controller/src/core.rs:574`) only marks the local cache and never calls
  `ActionsService/Abort`.
- **Waiting is resumable, and safe to abandon.** The completion channel is registered at
  `create()` and parked by action name, so a signal that lands before anyone calls `wait()` is
  not lost — the later `wait()` returns immediately. A dropped `wait()` (a `tokio::select!`
  loser) leaves the condition `PAUSED` rather than corrupting anything, and a retry of the task
  re-derives the same action name and picks up the already-signalled value.

### Composition

```rust
// Ask both questions up front so reviewers can answer in parallel, then collect.
let security = flyte::condition::<bool>("security-review").create().await?;
let legal = flyte::condition::<bool>("legal-review").create().await?;
let (ok_security, ok_legal) = futures::try_join!(security.wait(), legal.wait())?;

// Race an approval against a deadline. Set .timeout() too: abandoning the wait
// does not reap the condition, only the server-side timeout does.
let approval = flyte::condition::<bool>("approve")
    .timeout(Duration::from_secs(600))
    .create()
    .await?;
tokio::select! {
    approved = approval.wait() => approved?,
    _ = tokio::time::sleep(Duration::from_secs(300)) => false,
}
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
| `ACTION_PHASE_RECOVERED` | success-equivalent — the action was adopted from a prior run and did not execute here, but its value is valid. Use `Action::is_action_successful()` rather than testing `phase == Succeeded` or inverting `Failed`. |

### The `rs_controller` side is done

Shipped in **[flyteorg/flyte-sdk#1401](https://github.com/flyteorg/flyte-sdk/pull/1401)**, so
this is no longer a blocker — what remains is the SDK-side code below. All of it is additive;
task and trace behavior is unchanged.

- `ActionType::Condition`, plus `Action.condition` and `Action.condition_output`, and
  `Action::from_condition` (phase `Unspecified`, no outputs URI, `started = false`).
- `merge_update` and `new_from_update` now capture `ActionUpdate.value`. Un-gated on action
  type, because an entry created from a watch update is typed `Task` until a later submit
  repairs it — gating would drop the value of an already-signalled condition on a retry.
- `merge_from_submit` adopts `action_type` and the type-specific spec, so a condition whose
  update arrived before its submit is still recognised as one and gets enqueued.
- `Condition` arms in `build_actions_spec` and `launch_task`; the legacy `QueueService` arm
  errors clearly, since the backend has no condition support there.
- **`submit_action` split into `start_action` + `wait_for_action`** — this is what makes the
  decoupled `create()` / `wait()` above possible. Completion receivers are now parked by action
  name in the informer, so a wait can be claimed by a caller that did not submit the action;
  previously the receiver lived only on `submit_action`'s stack. `submit_action` is now their
  composition and behaves identically for tasks and traces.
- The trace wait bound became an explicit `match` over action type, so a future type must choose
  its policy. Conditions take the unbounded wait deliberately — they are bounded server-side by
  `ConditionAction.timeout`, which arrives as a `TIMED_OUT` update.
- `Action::is_action_successful()` treats **`Recovered`** as success-equivalent, and
  `is_action_paused()` names the condition wait. Both are exposed to Python
  (`is_successful` / `is_paused`), with `condition_output_bytes` for the value.

Still absent there, and worth knowing: `cancel_action` never calls `ActionsService/Abort`, so
an abandoned condition can only be reaped by its server-side timeout.

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
