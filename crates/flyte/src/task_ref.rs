//! Calling a task from a task: [`call_task`] launches another task as a child
//! action of the running one and waits for its outputs -- Python's
//! `await other_task(...)` inside a task.
//!
//! Mirrors `RemoteController._submit` / `_submit_task_ref`: the child's action
//! name, call sequence and cache key are computed exactly as Python computes
//! them, so a Rust parent and a Python parent name the same call the same way
//! and share its cache entries and recovery.
//!
//! The one thing Rust cannot build for itself is the child's `TaskSpec` (image,
//! args, resources, cache config...). Where it comes from is a [`SpecSource`]
//! held by the [`TaskRef`], resolved when the call is made -- never something a
//! call site spells out. Today a spec comes either from the bundle the Python
//! launcher hands the worker at deploy time (`rust_task(calls=[...])`), or from
//! serialized bytes the caller supplies. Fetching a deployed task by reference
//! or defining it natively in Rust are further sources, added without touching
//! any `call_task` call site.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, RwLock};

use base64::Engine as _;
use prost::Message as _;

use crate::context::{self, RuntimeState};
use crate::controller::{Controller, StartedTask, TaskFailure, TaskRecord, TaskResult};
use crate::error::Error;
use crate::hash;
use crate::idl::{
    ActionPhase, Inputs, Literal, NamedLiteral, Outputs, RetryStrategy, RunIdentifier, TaskSpec,
};
use crate::storage::Storage;
use crate::types::FlyteType;

/// A task that can be called with [`call_task`].
///
/// ```ignore
/// static COMPILE: flyte::TaskRef = flyte::TaskRef::new("ci_env.compile");
///
/// let out: String = flyte::call_task(&COMPILE, flyte::inputs! { pkg: pkg }).await?;
/// let out: String = flyte::call_task(&COMPILE.with_retries(3), flyte::inputs! { pkg: pkg }).await?;
/// ```
#[derive(Clone, Debug)]
pub struct TaskRef {
    source: SpecSource,
    overrides: Overrides,
}

/// Where a [`TaskRef`]'s `TaskSpec` comes from. More sources will be added, so
/// match on it with a wildcard arm.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum SpecSource {
    /// A task named by its fully-qualified name (`"{env}.{task}"`), resolved
    /// when it is called from the specs this worker was deployed with -- see
    /// [`register_spec_bundle`].
    Named(Cow<'static, str>),
    /// A spec supplied directly, already decoded.
    Inline(Arc<ResolvedSpec>),
}

/// Python's `task.override(...)`: per-call changes that do not alter the task's
/// identity, so the call is still named and cached as the task itself.
#[derive(Clone, Debug, Default)]
struct Overrides {
    retries: Option<u32>,
    short_name: Option<Cow<'static, str>>,
}

impl TaskRef {
    /// The task `name`, qualified by its environment: `"{env}.{task}"`, e.g.
    /// `"ci_env.compile"`. The bare task name also resolves when no other
    /// callable environment has a task of that name.
    /// `const`, so a task can be declared once as a `static`.
    pub const fn new(name: &'static str) -> Self {
        TaskRef {
            source: SpecSource::Named(Cow::Borrowed(name)),
            overrides: Overrides {
                retries: None,
                short_name: None,
            },
        }
    }

    /// [`Self::new`] for a name only known at runtime.
    pub fn named(name: impl Into<String>) -> Self {
        Self::from_source(SpecSource::Named(Cow::Owned(name.into())))
    }

    /// A task from a serialized `flyteidl2.task.TaskSpec`.
    ///
    /// Inputs are hashed in the order the spec's interface lists them, which is
    /// what Python does for a task reference. A task declared with its inputs
    /// in some other order hashes differently from a Python parent calling the
    /// task object directly; the launcher bundle carries the declared order for
    /// exactly that reason.
    pub fn from_spec_bytes(spec: &[u8]) -> Result<Self, Error> {
        Ok(Self::from_source(SpecSource::Inline(Arc::new(
            ResolvedSpec::decode(spec, None)?,
        ))))
    }

    pub fn from_source(source: SpecSource) -> Self {
        TaskRef {
            source,
            overrides: Overrides::default(),
        }
    }

    pub fn source(&self) -> &SpecSource {
        &self.source
    }

    /// Retry the task up to `retries` times -- `override(retries=...)`.
    pub fn with_retries(&self, retries: u32) -> Self {
        let mut t = self.clone();
        t.overrides.retries = Some(retries);
        t
    }

    /// Display name of the call in the UI -- `override(short_name=...)`.
    pub fn with_short_name(&self, short_name: impl Into<String>) -> Self {
        let mut t = self.clone();
        t.overrides.short_name = Some(Cow::Owned(short_name.into()));
        t
    }

    fn resolve(&self) -> Result<Arc<ResolvedSpec>, Error> {
        match &self.source {
            SpecSource::Inline(spec) => Ok(spec.clone()),
            SpecSource::Named(name) => lookup_spec(name),
        }
    }

    fn display_name(&self) -> &str {
        match &self.source {
            SpecSource::Named(name) => name,
            SpecSource::Inline(spec) => spec.name(),
        }
    }
}

/// A decoded `TaskSpec` plus the hashes every call to it needs.
#[derive(Debug)]
pub struct ResolvedSpec {
    spec: TaskSpec,
    /// Order inputs are serialized and hashed in: the task's declared order
    /// when known, otherwise its interface's (sorted) order.
    input_order: Vec<String>,
    interface_hash: String,
    identity: String,
}

impl ResolvedSpec {
    /// Decode a serialized `TaskSpec`. `input_order` is the task's declared
    /// input order, when the producer knows it.
    pub fn decode(bytes: &[u8], input_order: Option<Vec<String>>) -> Result<Self, Error> {
        let spec = TaskSpec::decode(bytes)?;
        let template = spec
            .task_template
            .as_ref()
            .ok_or_else(|| Error::System("task spec has no task_template".into()))?;
        let iface = template.interface.clone().unwrap_or_default();
        let declared: Vec<String> = iface
            .inputs
            .iter()
            .flat_map(|m| m.variables.iter().map(|v| v.key.clone()))
            .collect();
        let input_order = match input_order {
            Some(order) => {
                let mut a = order.clone();
                let mut b = declared.clone();
                a.sort();
                b.sort();
                if a != b {
                    return Err(Error::System(format!(
                        "declared input order {order:?} does not match the interface's inputs {declared:?}"
                    )));
                }
                order
            }
            None => declared,
        };
        // TaskSpec.task_template (1) -> TaskTemplate.interface (4).
        let raw_iface = wire_field(bytes, 1).and_then(|t| wire_field(t, 4));
        let interface_hash = hash::interface_hash(&iface, raw_iface);
        let identity = hash::task_identity_hash(template, &interface_hash);
        Ok(ResolvedSpec {
            spec,
            input_order,
            interface_hash,
            identity,
        })
    }

    /// The task's fully-qualified name.
    pub fn name(&self) -> &str {
        self.spec
            .task_template
            .as_ref()
            .and_then(|t| t.id.as_ref())
            .map_or("", |id| id.name.as_str())
    }

    /// `"{environment}.{task}"`: the name a [`TaskRef`] uses. Python prefixes
    /// a function task's own name with its environment already, so that is
    /// kept as is; a `rust_task` keeps its bare name (`id.name`, which is what
    /// the backend registers and the cache key hashes), and gains the prefix
    /// here only.
    pub fn qualified_name(&self) -> String {
        let name = self.name();
        match self.spec.environment.as_ref().map(|e| e.name.as_str()) {
            Some(env) if !env.is_empty() && !name.starts_with(&format!("{env}.")) => {
                format!("{env}.{name}")
            }
            _ => name.to_string(),
        }
    }

    fn has_outputs(&self) -> bool {
        self.spec
            .task_template
            .as_ref()
            .and_then(|t| t.interface.as_ref())
            .and_then(|i| i.outputs.as_ref())
            .is_some_and(|o| !o.variables.is_empty())
    }
}

/// The last occurrence of length-delimited field `tag` in a serialized message
/// (protobuf's last-one-wins rule for singular fields), without decoding it.
fn wire_field(mut buf: &[u8], tag: u32) -> Option<&[u8]> {
    fn varint(buf: &mut &[u8]) -> Option<u64> {
        let mut v = 0u64;
        for shift in (0..64).step_by(7) {
            let (&b, rest) = buf.split_first()?;
            *buf = rest;
            v |= u64::from(b & 0x7f) << shift;
            if b & 0x80 == 0 {
                return Some(v);
            }
        }
        None
    }
    let mut found = None;
    while !buf.is_empty() {
        let key = varint(&mut buf)?;
        let len = match key & 7 {
            0 => {
                varint(&mut buf)?;
                0
            }
            1 => 8,
            2 => varint(&mut buf)? as usize,
            5 => 4,
            _ => return None,
        };
        if len > buf.len() {
            return None;
        }
        let (field, rest) = buf.split_at(len);
        if key & 7 == 2 && (key >> 3) as u32 == tag {
            found = Some(field);
        }
        buf = rest;
    }
    found
}

// ---------------------------------------------------------------------------
// Spec bundle: the child specs the launcher serialized at deploy time.

/// Keyed by [`ResolvedSpec::qualified_name`].
static SPECS: LazyLock<RwLock<HashMap<String, Arc<ResolvedSpec>>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// Env var a bundle may arrive in instead of the `--task-specs` arg.
pub const SPEC_BUNDLE_ENV: &str = "_F_RS_TASK_SPECS";

#[derive(serde::Deserialize)]
struct Bundle {
    /// Format version; this SDK reads 1.
    v: u32,
    tasks: Vec<BundleEntry>,
}

#[derive(serde::Deserialize)]
struct BundleEntry {
    /// base64 of a serialized `TaskSpec`.
    spec: String,
    /// The task's declared input order.
    #[serde(default)]
    inputs: Option<Vec<String>>,
}

/// Register the specs in a launcher bundle -- base64 of JSON
/// `{"v": 1, "tasks": [{"spec": <base64 TaskSpec>, "inputs": [<declared input names>]}]}`
/// -- making them callable by name. Returns how many it registered.
///
/// The worker does this for the `--task-specs` argument, so neither `#[flyte::main]`
/// nor a reusable container's entrypoint has to.
#[doc(hidden)]
pub fn register_spec_bundle(bundle: &str) -> Result<usize, Error> {
    let b64 = base64::engine::general_purpose::STANDARD;
    let bad = |e: &dyn std::fmt::Display| Error::System(format!("bad task spec bundle: {e}"));
    let json = b64.decode(bundle.trim()).map_err(|e| bad(&e))?;
    let Bundle { v, tasks: entries } = serde_json::from_slice(&json).map_err(|e| bad(&e))?;
    if v != 1 {
        return Err(Error::System(format!(
            "task spec bundle version {v} is newer than this SDK reads (1): upgrade flyte"
        )));
    }
    let mut specs = SPECS.write().expect("spec registry poisoned");
    for entry in &entries {
        let bytes = b64.decode(&entry.spec).map_err(|e| bad(&e))?;
        let spec = ResolvedSpec::decode(&bytes, entry.inputs.clone())?;
        specs.insert(spec.qualified_name(), Arc::new(spec));
    }
    Ok(entries.len())
}

fn lookup_spec(name: &str) -> Result<Arc<ResolvedSpec>, Error> {
    static FROM_ENV: std::sync::Once = std::sync::Once::new();
    FROM_ENV.call_once(|| {
        if let Ok(bundle) = std::env::var(SPEC_BUNDLE_ENV)
            && !bundle.is_empty()
            && let Err(e) = register_spec_bundle(&bundle)
        {
            tracing::error!("{SPEC_BUNDLE_ENV}: {e}");
        }
    });
    let specs = SPECS.read().expect("spec registry poisoned");
    if let Some(spec) = specs.get(name) {
        return Ok(spec.clone());
    }
    // The bare task name, when only one environment has a task by that name.
    let mut bare = specs.iter().filter(|(_, s)| s.name() == name);
    match (bare.next(), bare.next()) {
        (Some((_, spec)), None) => return Ok(spec.clone()),
        (Some((a, _)), Some((b, _))) => {
            return Err(Error::System(format!(
                "task name {name:?} is ambiguous: qualify it with its environment, e.g. {a:?} or {b:?}"
            )));
        }
        _ => {}
    }
    let mut known: Vec<_> = specs.keys().cloned().collect();
    known.sort();
    Err(Error::System(format!(
        "task {name:?} is not callable from this worker: no spec was deployed with it \
         (declare it in the launcher with rust_task(calls=[...])). Callable tasks: {known:?}"
    )))
}

// ---------------------------------------------------------------------------
// Inputs and outputs.

/// Named inputs for [`call_task`]; build with [`inputs!`](crate::inputs).
/// Conversion errors are deferred to the call, so building never needs `?`.
#[derive(Default)]
pub struct TaskInputs {
    literals: Vec<(String, Literal)>,
    error: Option<Error>,
}

impl TaskInputs {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add input `name`. Order does not matter: inputs are put in the task's own
    /// order when the call is made.
    pub fn arg<T: IntoInput + ?Sized>(self, name: &str, value: &T) -> Self {
        self.arg_literal(name, value.to_input())
    }

    #[doc(hidden)]
    pub fn arg_literal(mut self, name: &str, value: Result<Literal, Error>) -> Self {
        if self.error.is_none() {
            match value {
                Ok(lit) => self.literals.push((name.to_string(), lit)),
                Err(e) => self.error = Some(e),
            }
        }
        self
    }

    /// Order and check against the task's inputs: every input exactly once.
    fn into_inputs(self, task: &str, spec: &ResolvedSpec) -> Result<Inputs, Error> {
        if let Some(e) = self.error {
            return Err(e);
        }
        let mut given: HashMap<String, Literal> = HashMap::new();
        for (name, lit) in self.literals {
            if !spec.input_order.contains(&name) {
                return Err(Error::Type(format!(
                    "task {task} has no input {name:?} (inputs: {:?})",
                    spec.input_order
                )));
            }
            if given.insert(name.clone(), lit).is_some() {
                return Err(Error::Type(format!(
                    "input {name:?} given twice for {task}"
                )));
            }
        }
        let defaults: HashMap<&str, &Literal> = spec
            .spec
            .default_inputs
            .iter()
            .filter_map(|p| {
                let lit = p.parameter.as_ref()?.behavior.as_ref()?;
                match lit {
                    crate::idl::parameter::Behavior::Default(l) => Some((p.name.as_str(), l)),
                    _ => None,
                }
            })
            .collect();
        let mut literals = Vec::with_capacity(spec.input_order.len());
        for name in &spec.input_order {
            let value = match given.remove(name) {
                Some(v) => v,
                None => match defaults.get(name.as_str()) {
                    Some(&d) => d.clone(),
                    None => {
                        return Err(Error::Type(format!(
                            "missing required input {name:?} for task {task}"
                        )));
                    }
                },
            };
            literals.push(NamedLiteral {
                name: name.clone(),
                value: Some(value),
            });
        }
        Ok(Inputs {
            literals,
            ..Default::default()
        })
    }
}

/// A value usable as a task input: any [`FlyteType`], or `str`. [`inputs!`](crate::inputs)
/// also takes references to these, however deep, by calling `to_input` with
/// method syntax and so auto-dereferencing.
#[doc(hidden)]
pub trait IntoInput {
    fn to_input(&self) -> Result<Literal, Error>;
}

impl<T: FlyteType> IntoInput for T {
    fn to_input(&self) -> Result<Literal, Error> {
        self.to_literal()
    }
}

impl IntoInput for str {
    fn to_input(&self) -> Result<Literal, Error> {
        self.to_string().to_literal()
    }
}

/// Build [`TaskInputs`] for [`call_task`]: `flyte::inputs! { pkg: &pkg, jobs: 4 }`.
#[macro_export]
macro_rules! inputs {
    ($($name:ident : $value:expr),* $(,)?) => {{
        #[allow(unused_imports)]
        use $crate::task_ref::IntoInput as _;
        let __inputs = $crate::TaskInputs::new();
        $( let __inputs = __inputs.arg_literal(::core::stringify!($name), ($value).to_input()); )*
        __inputs
    }};
}

/// What a [`call_task`] returns: `()` for a task without outputs, any
/// [`FlyteType`] for a task's single output `o0`, or [`TaskOutputs`] for all of
/// them.
pub trait TaskOutput: Sized {
    #[doc(hidden)]
    fn from_outputs(outputs: Outputs) -> Result<Self, Error>;
}

impl TaskOutput for () {
    fn from_outputs(_: Outputs) -> Result<Self, Error> {
        Ok(())
    }
}

impl<T: FlyteType> TaskOutput for T {
    fn from_outputs(outputs: Outputs) -> Result<Self, Error> {
        T::from_literal(crate::types::output_literal(&outputs, "o0")?)
    }
}

/// All of a task's outputs, by name.
pub struct TaskOutputs(Outputs);

impl TaskOutputs {
    pub fn get<T: FlyteType>(&self, name: &str) -> Result<T, Error> {
        T::from_literal(crate::types::output_literal(&self.0, name)?)
    }
}

impl TaskOutput for TaskOutputs {
    fn from_outputs(outputs: Outputs) -> Result<Self, Error> {
        Ok(TaskOutputs(outputs))
    }
}

// ---------------------------------------------------------------------------
// The call.

/// Bounds how many calls upload inputs and enqueue at once, so a wide fan-out
/// does not open thousands of storage requests together. Waiting is not
/// bounded: a waiting call costs nothing but its future.
static SUBMIT_PERMITS: LazyLock<tokio::sync::Semaphore> = LazyLock::new(|| {
    let n = std::env::var("_F_MAX_SUBMIT_CONCURRENCY")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(64);
    tokio::sync::Semaphore::new(n)
});

/// Launch `task` as a child action of the running task and wait for its result.
///
/// The call is named deterministically from the calling action, the task's
/// identity, the inputs, its position among identical calls and the current
/// [`group`](crate::group), so a retried or recovered parent finds the calls
/// its previous attempt made rather than repeating them. When the task is
/// cached, a call with a cache hit completes without running.
///
/// Calls are ordinary futures: run them concurrently with `join!` or
/// `try_join_all`. Dropping one before it finishes cancels the child action --
/// for now only on this side: see [`Controller::cancel_task`].
///
/// Outside a running task (e.g. under [`run`](crate::run)) there is no backend
/// to launch the child on: the call runs the [`stub_task`] registered for it,
/// and fails if there is none.
pub async fn call_task<O: TaskOutput>(task: &TaskRef, inputs: TaskInputs) -> Result<O, Error> {
    let Some(state) = context::current() else {
        return O::from_outputs(call_stub(task.display_name(), inputs).await?);
    };
    let resolved = task.resolve()?;
    let name = resolved.name().to_string();
    let inputs = inputs.into_inputs(&name, &resolved)?;
    let outputs = launch(&state, task, &resolved, inputs).await?;
    O::from_outputs(outputs)
}

// ---------------------------------------------------------------------------
// Local stubs: what a call does with no backend.

type StubFn = Arc<
    dyn Fn(StubInputs) -> futures::future::BoxFuture<'static, Result<Outputs, Error>> + Send + Sync,
>;

static STUBS: LazyLock<RwLock<HashMap<String, StubFn>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// The inputs a [`stub_task`] was called with.
pub struct StubInputs(HashMap<String, Literal>);

impl StubInputs {
    pub fn get<T: FlyteType>(&self, name: &str) -> Result<T, Error> {
        let lit = self
            .0
            .get(name)
            .ok_or_else(|| Error::Type(format!("stub called without input {name:?}")))?;
        T::from_literal(lit)
    }
}

/// What a [`stub_task`] returns: `()`, or any [`FlyteType`] as output `o0`.
pub trait StubOutput {
    #[doc(hidden)]
    fn into_outputs(self) -> Result<Outputs, Error>;
}

impl StubOutput for () {
    fn into_outputs(self) -> Result<Outputs, Error> {
        Ok(Outputs::default())
    }
}

impl<T: FlyteType> StubOutput for T {
    fn into_outputs(self) -> Result<Outputs, Error> {
        Ok(crate::types::build_outputs(vec![(
            "o0",
            self.to_literal()?,
        )]))
    }
}

/// Stand in for task `name` when it is called with no backend -- in tests and
/// under [`run`](crate::run) -- since the real task, possibly another binary,
/// cannot run in-process. Replaces any stub already registered for `name`;
/// stubs are process-wide, so tests that stub the same task differently must
/// not run concurrently.
///
/// ```ignore
/// flyte::stub_task("ci_env.compile", |i| async move {
///     Ok(format!("built {}", i.get::<String>("pkg")?))
/// });
/// ```
pub fn stub_task<O, F, Fut>(name: &str, f: F)
where
    O: StubOutput,
    F: Fn(StubInputs) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Result<O, Error>> + Send + 'static,
{
    let f: StubFn = Arc::new(move |inputs| {
        let fut = f(inputs);
        Box::pin(async move { fut.await?.into_outputs() })
    });
    STUBS
        .write()
        .expect("stub registry poisoned")
        .insert(name.to_string(), f);
}

async fn call_stub(name: &str, inputs: TaskInputs) -> Result<Outputs, Error> {
    if let Some(e) = inputs.error {
        return Err(e);
    }
    let stub = STUBS
        .read()
        .expect("stub registry poisoned")
        .get(name)
        .cloned()
        .ok_or_else(|| {
            Error::System(format!(
                "call_task({name}) has no backend to launch it on: run it inside a task, \
                 or register a local stand-in with flyte::stub_task"
            ))
        })?;
    stub(StubInputs(inputs.literals.into_iter().collect())).await
}

/// What [`launch`] computes before submitting: everything that must match
/// Python byte-for-byte.
#[derive(Debug, PartialEq)]
struct CallNames {
    action_name: String,
    cache_key: Option<String>,
}

fn call_names(
    resolved: &ResolvedSpec,
    inputs: &Inputs,
    parent: &str,
    group: Option<&str>,
    next_seq: impl FnOnce(&str) -> u32,
) -> CallNames {
    let metadata = resolved
        .spec
        .task_template
        .as_ref()
        .and_then(|t| t.metadata.clone())
        .unwrap_or_default();
    let ignored = &metadata.cache_ignore_input_vars;
    let inputs_hash = hash::inputs_hash(inputs);
    let name_inputs_hash = if ignored.is_empty() {
        inputs_hash.clone()
    } else {
        hash::filtered_inputs_hash(inputs, ignored)
    };
    let seq = next_seq(&context::sequence_key(
        &format!("{}:{name_inputs_hash}", resolved.identity),
        group,
    ));
    let action_name =
        hash::sub_action_name(parent, &name_inputs_hash, &resolved.identity, seq, group);
    let cache_key = metadata.discoverable.then(|| {
        hash::cache_key_hash(
            resolved.name(),
            inputs,
            &inputs_hash,
            &resolved.interface_hash,
            &metadata.discovery_version,
            ignored,
        )
    });
    CallNames {
        action_name,
        cache_key,
    }
}

/// The spec actually submitted for one call: the resolved spec placed in the
/// calling run's org/project/domain -- Python builds a callee's spec under the
/// current action's, and a spec serialized at deploy time has none, which the
/// backend rejects -- with the call's overrides applied. Neither touches
/// anything the action name or cache key hashes (`id.name`, the interface,
/// `discovery_version`).
fn spec_for_call(spec: &TaskSpec, overrides: &Overrides, run: &RunIdentifier) -> TaskSpec {
    let mut spec = spec.clone();
    if let Some(template) = spec.task_template.as_mut() {
        if let Some(id) = template.id.as_mut() {
            id.org.clone_from(&run.org);
            id.project.clone_from(&run.project);
            id.domain.clone_from(&run.domain);
        }
        if let Some(retries) = overrides.retries
            && let Some(md) = template.metadata.as_mut()
        {
            md.retries = Some(RetryStrategy {
                retries,
                ..Default::default()
            });
        }
    }
    if let Some(short_name) = &overrides.short_name {
        spec.short_name = short_name.to_string();
    }
    spec
}

async fn launch(
    state: &RuntimeState,
    task: &TaskRef,
    resolved: &ResolvedSpec,
    inputs: Inputs,
) -> Result<Outputs, Error> {
    let task_name = resolved.name();
    let group = context::current_group();
    let CallNames {
        action_name,
        cache_key,
    } = call_names(
        resolved,
        &inputs,
        &state.action_name,
        group.as_deref(),
        |key| state.next_seq(key),
    );

    let spec = spec_for_call(&resolved.spec, &task.overrides, state.controller.run_id());

    let started = {
        let _permit = SUBMIT_PERMITS
            .acquire()
            .await
            .map_err(|e| Error::System(e.to_string()))?;
        let inputs_uri = Storage::join(
            &Storage::join(&state.run_base_dir, &action_name),
            "inputs.pb",
        );
        state
            .storage
            .put(&inputs_uri, bytes::Bytes::from(inputs.encode_to_vec()))
            .await?;
        tracing::info!(task = task_name, action = %action_name, "submitting task action");
        state
            .controller
            .start_task(TaskRecord {
                parent_action_name: state.action_name.clone(),
                action_name: action_name.clone(),
                spec,
                group,
                inputs_uri,
                run_output_base: state.run_base_dir.clone(),
                cache_key,
            })
            .await?
    };

    let mut guard = CancelOnDrop(Some((state.controller.clone(), started)));
    let result = state
        .controller
        .wait_task(&state.action_name, &action_name)
        .await;
    guard.0 = None;

    match result? {
        TaskResult::Succeeded { outputs_uri } => match outputs_uri {
            // A prefix, as for Python's `io.outputs_path(realized_outputs_uri)`.
            Some(uri) => {
                let data = state
                    .storage
                    .get(&Storage::join(&uri, "outputs.pb"))
                    .await?;
                Ok(Outputs::decode(data.as_ref())?)
            }
            None if resolved.has_outputs() => Err(Error::System(format!(
                "task {task_name} (action {action_name}) succeeded without an output path, \
                 but it declares outputs"
            ))),
            None => Ok(Outputs::default()),
        },
        TaskResult::Failed(failure) => Err(failure_error(task_name, &action_name, failure)),
    }
}

/// The error a failed child surfaces as in its caller, with the Python SDK's
/// codes so it reads the same whichever SDK called it.
fn failure_error(task: &str, action: &str, f: TaskFailure) -> Error {
    let message = format!("task {task} (action {action}) failed: {}", f.message);
    match f.phase {
        Some(ActionPhase::Aborted) => Error::User {
            code: "ActionAbortedError".into(),
            message,
        },
        Some(ActionPhase::TimedOut) => Error::User {
            code: "TaskTimeoutError".into(),
            message,
        },
        _ if f.user => Error::User {
            code: if f.code.is_empty() {
                "TaskFailed".into()
            } else {
                f.code
            },
            message,
        },
        _ => Error::System(if f.code.is_empty() {
            message
        } else {
            format!("{}: {message}", f.code)
        }),
    }
}

/// Cancels a started child when its caller stops waiting (the call's future was
/// dropped), as Python cancels on `CancelledError`.
struct CancelOnDrop(Option<(Controller, StartedTask)>);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        if let Some((controller, task)) = self.0.take()
            && let Ok(handle) = tokio::runtime::Handle::try_current()
        {
            handle.spawn(async move {
                if let Err(e) = controller.cancel_task(task).await {
                    tracing::warn!("cancelling abandoned task action failed: {e}");
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_field_finds_nested_fields() {
        let spec = TaskSpec {
            short_name: "s".into(),
            task_template: Some(crate::idl::TaskTemplate {
                r#type: "t".into(),
                interface: Some(crate::types::build_typed_interface(
                    &[("a", <String as FlyteType>::literal_type())],
                    &[],
                )),
                ..Default::default()
            }),
            ..Default::default()
        };
        let bytes = spec.encode_to_vec();
        let iface = wire_field(wire_field(&bytes, 1).unwrap(), 4).unwrap();
        let expected = spec
            .task_template
            .unwrap()
            .interface
            .unwrap()
            .encode_to_vec();
        assert_eq!(iface, expected.as_slice());
        assert_eq!(wire_field(&bytes, 9), None);
    }

    fn unhex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    /// Golden values from the Python SDK itself (`scripts/gen_call_task_golden.py`):
    /// a real serialized spec, and the action name and cache key Python's
    /// `_submit` derives for a call to it. Inputs are deliberately given out of
    /// order -- the declared order must win.
    #[test]
    fn call_names_match_python() {
        let golden: Vec<serde_json::Value> =
            serde_json::from_str(include_str!("../tests/golden/call_task.json")).unwrap();
        let calls = [
            (
                Some(vec!["pkg".into(), "sha".into(), "jobs".into()]),
                crate::inputs! { jobs: 4i64, sha: "deadbeef", pkg: "go/pkg/a" },
            ),
            (
                Some(vec!["pkg".into(), "sha".into(), "jobs".into()]),
                crate::inputs! { sha: "cafef00d", jobs: 4i64, pkg: &&String::from("go/pkg/a") },
            ),
            (None, crate::inputs! { pkg: "x" }),
        ];
        assert_eq!(golden.len(), calls.len());
        for (g, (order, inputs)) in golden.iter().zip(calls) {
            let s = |k: &str| g[k].as_str().map(str::to_string);
            let spec = ResolvedSpec::decode(&unhex(&s("task_spec_hex").unwrap()), order).unwrap();
            assert_eq!(Some(spec.interface_hash.clone()), s("interface_hash"));
            assert_eq!(Some(spec.identity.clone()), s("identity"));

            let inputs = inputs.into_inputs(spec.name(), &spec).unwrap();
            assert_eq!(inputs.encode_to_vec(), unhex(&s("inputs_hex").unwrap()));

            let seq = g["seq"].as_u64().unwrap() as u32;
            let names = call_names(
                &spec,
                &inputs,
                &s("parent").unwrap(),
                s("group").as_deref(),
                |_| seq,
            );
            assert_eq!(
                names,
                CallNames {
                    action_name: s("action_name").unwrap(),
                    cache_key: s("cache_key"),
                }
            );
        }
    }

    #[test]
    fn calls_are_placed_in_the_callers_run_without_changing_names() {
        let golden: Vec<serde_json::Value> =
            serde_json::from_str(include_str!("../tests/golden/call_task.json")).unwrap();
        let mut deployed =
            TaskSpec::decode(unhex(golden[0]["task_spec_hex"].as_str().unwrap()).as_slice())
                .unwrap();
        // As serialized at deploy time: no org/project/domain.
        let id = deployed
            .task_template
            .as_mut()
            .unwrap()
            .id
            .as_mut()
            .unwrap();
        (id.org, id.project, id.domain) = Default::default();
        let run = RunIdentifier {
            org: "o2".into(),
            project: "adam".into(),
            domain: "development".into(),
            name: "r".into(),
        };
        let overrides = Overrides {
            retries: Some(3),
            short_name: Some("compile-x".into()),
        };
        let called = spec_for_call(&deployed, &overrides, &run);
        let template = called.task_template.as_ref().unwrap();
        let id = template.id.as_ref().unwrap();
        assert_eq!(
            (id.org.as_str(), id.project.as_str(), id.domain.as_str()),
            ("o2", "adam", "development")
        );
        assert_eq!(id.name, "golden_env.compile");
        assert_eq!(
            template
                .metadata
                .as_ref()
                .unwrap()
                .retries
                .as_ref()
                .unwrap()
                .retries,
            3
        );
        assert_eq!(called.short_name, "compile-x");
        // Identity and interface hash are the same whichever run it lands in.
        let a = ResolvedSpec::decode(&deployed.encode_to_vec(), None).unwrap();
        let b = ResolvedSpec::decode(&called.encode_to_vec(), None).unwrap();
        assert_eq!(
            (&a.identity, &a.interface_hash),
            (&b.identity, &b.interface_hash)
        );
        assert_eq!(a.identity, golden[0]["identity"].as_str().unwrap());
    }

    #[test]
    fn inputs_are_checked_against_the_interface() {
        let golden: Vec<serde_json::Value> =
            serde_json::from_str(include_str!("../tests/golden/call_task.json")).unwrap();
        let spec = ResolvedSpec::decode(&unhex(golden[0]["task_spec_hex"].as_str().unwrap()), None)
            .unwrap();
        let err = |i: TaskInputs| i.into_inputs("t", &spec).unwrap_err().to_string();
        assert!(
            err(crate::inputs! { pkg: "a", sha: "b" }).contains("missing required input \"jobs\"")
        );
        assert!(
            err(crate::inputs! { pkg: "a", sha: "b", jobs: 1i64, nope: 1i64 })
                .contains("no input \"nope\"")
        );
        assert!(
            err(crate::inputs! { pkg: "a", pkg: "a", sha: "b", jobs: 1i64 })
                .contains("given twice")
        );
        assert!(
            ResolvedSpec::decode(
                &unhex(golden[0]["task_spec_hex"].as_str().unwrap()),
                Some(vec!["pkg".into()])
            )
            .is_err()
        );
    }

    #[test]
    fn bundles_register_by_task_name() {
        let golden: Vec<serde_json::Value> =
            serde_json::from_str(include_str!("../tests/golden/call_task.json")).unwrap();
        let b64 = base64::engine::general_purpose::STANDARD;
        let mut entries: Vec<_> = golden
            .iter()
            .map(|g| serde_json::json!({ "spec": b64.encode(unhex(g["task_spec_hex"].as_str().unwrap())) }))
            .collect();
        // Shaped like rust_task specs: a bare task name, the environment beside
        // it; and the same bare name in two environments.
        let lint = TaskSpec::decode(unhex(golden[2]["task_spec_hex"].as_str().unwrap()).as_slice())
            .unwrap();
        for env in ["rs-step", "rs-other"] {
            let mut spec = lint.clone();
            let template = spec.task_template.as_mut().unwrap();
            template.id.as_mut().unwrap().name = if env == "rs-step" {
                "step_rs"
            } else {
                "dup_rs"
            }
            .into();
            spec.environment.as_mut().unwrap().name = env.into();
            entries.push(serde_json::json!({ "spec": b64.encode(spec.encode_to_vec()) }));
            let mut dup = spec.clone();
            dup.task_template
                .as_mut()
                .unwrap()
                .id
                .as_mut()
                .unwrap()
                .name = "dup_rs".into();
            entries.push(serde_json::json!({ "spec": b64.encode(dup.encode_to_vec()) }));
        }
        let bundle =
            b64.encode(serde_json::to_vec(&serde_json::json!({"v": 1, "tasks": entries})).unwrap());
        assert_eq!(register_spec_bundle(&bundle).unwrap(), 7);
        let resolve = |n: &str| TaskRef::named(n).resolve().map(|s| s.name().to_string());
        assert_eq!(resolve("rs-step.step_rs").unwrap(), "step_rs");
        assert_eq!(resolve("step_rs").unwrap(), "step_rs");
        assert_eq!(resolve("rs-other.dup_rs").unwrap(), "dup_rs");
        let err = resolve("dup_rs").unwrap_err().to_string();
        assert!(
            err.contains("ambiguous") && err.contains("rs-step.dup_rs"),
            "{err}"
        );
        assert_eq!(
            TaskRef::new("golden_env.lint").resolve().unwrap().name(),
            "golden_env.lint"
        );
        let err = TaskRef::new("golden_env.nope")
            .resolve()
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("rust_task(calls=[...])") && err.contains("golden_env.compile"),
            "{err}"
        );
    }

    #[test]
    fn call_task_outside_a_task_runs_its_stub() {
        let err = crate::run(call_task::<()>(
            &TaskRef::new("x.unstubbed"),
            TaskInputs::new(),
        ))
        .unwrap_err();
        assert!(err.to_string().contains("flyte::stub_task"), "{err}");

        stub_task("x.compile", |i| async move {
            Ok(format!("built {}", i.get::<String>("pkg")?))
        });
        let out: String = crate::run(call_task(
            &TaskRef::new("x.compile").with_retries(2),
            crate::inputs! { pkg: "a" },
        ))
        .unwrap();
        assert_eq!(out, "built a");
    }
}
