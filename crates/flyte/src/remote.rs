//! Reading runs from the backend: list a project's runs and fetch an action's
//! inputs and outputs — what a task needs to find, say, the newest successful
//! run of some task and pick up what it returned.
//!
//! Mirrors `flyte.remote.Run.listall` and `ActionDetails.inputs/outputs` from
//! the Python SDK: the same `RunService` RPCs, the same filter conventions
//! (`phase`, `task_name`, `labels.<key>`), so a query reads the same in either
//! SDK. Like [`controller`](crate::controller), this is a swap-boundary file:
//! the transport (auth, channel) comes from `flyte_controller_base` and the
//! API speaks only SDK types plus the protos re-exported in [`crate::idl`].

use std::collections::BTreeMap;
use std::sync::Arc;

use flyte_controller_base::auth::{AuthConfig, AuthLayer, ClientCredentialsAuthenticator};
use flyte_controller_base::core::create_tls_channel;
use flyteidl2::flyteidl::common::{
    ActionIdentifier, ActionPhase, Filter, ListRequest, ProjectIdentifier, RunIdentifier, Sort,
    filter, sort,
};
use flyteidl2::flyteidl::workflow::run_service_client::RunServiceClient;
use flyteidl2::flyteidl::workflow::{GetActionDataRequest, ListRunsRequest, list_runs_request};
use tonic::transport::Channel;
use tower::ServiceBuilder;

use crate::error::Error;
use crate::idl::{Inputs, Outputs};

type AuthChannel = flyte_controller_base::auth::AuthService<Channel>;

#[derive(Clone)]
enum Client {
    Plain(RunServiceClient<Channel>),
    Authenticated(RunServiceClient<AuthChannel>),
}

/// A client for the backend's `RunService`.
///
/// Built from the same environment a task container gets (see
/// [`Runs::from_env`]), so inside a task it needs no configuration.
#[derive(Clone)]
pub struct Runs {
    client: Client,
    org: String,
}

/// Which runs to list. Everything is optional; unset fields do not filter.
#[derive(Debug, Clone, Default)]
pub struct RunQuery {
    pub project: String,
    pub domain: String,
    /// Only runs in one of these phases.
    pub phases: Vec<ActionPhase>,
    /// Only runs of this task (`<env>.<fn>`, as registered).
    pub task_name: Option<String>,
    /// Only runs carrying ALL of these labels with exactly these values.
    pub labels: BTreeMap<String, String>,
    /// Only runs that have all of these label keys, whatever their values.
    pub label_keys: Vec<String>,
    /// Newest first (by creation time) when true; oldest first otherwise.
    pub newest_first: bool,
    /// Page size; 0 means the backend's default.
    pub limit: u32,
}

/// One listed run: what `ListRuns` returns, flattened.
#[derive(Debug, Clone)]
pub struct RunInfo {
    pub name: String,
    pub phase: ActionPhase,
    /// Start time of the run's root action, unix seconds (0 if unknown).
    pub start_time: f64,
    pub labels: BTreeMap<String, String>,
    /// The root action, for [`Runs::action_data`].
    pub action: ActionIdentifier,
}

impl Runs {
    /// Connect the way the worker does: with `_UNION_EAGER_API_KEY` set (a task
    /// pod), an authenticated TLS channel to the endpoint inside the key;
    /// otherwise `_U_EP_OVERRIDE` without auth (a local sandbox).
    pub async fn from_env() -> Result<Self, Error> {
        let org = std::env::var("_U_ORG_NAME").unwrap_or_default();
        if let Some(key) = env_nonempty("_UNION_EAGER_API_KEY") {
            let config =
                AuthConfig::new_from_api_key(&key).map_err(|e| Error::Controller(e.to_string()))?;
            let endpoint: &'static str = Box::leak(config.endpoint.clone().into_boxed_str());
            let channel = create_tls_channel(endpoint)
                .await
                .map_err(|e| Error::Controller(e.to_string()))?;
            let authenticator = Arc::new(ClientCredentialsAuthenticator::new(config));
            let svc = ServiceBuilder::new()
                .layer(AuthLayer::new(authenticator, channel.clone()))
                .service(channel);
            return Ok(Self {
                client: Client::Authenticated(RunServiceClient::new(svc)),
                org,
            });
        }
        let raw = env_nonempty("_U_EP_OVERRIDE").unwrap_or_else(|| "http://localhost:8090".into());
        let url = if raw.contains("://") {
            raw
        } else {
            format!("http://{raw}")
        };
        let channel = tonic::transport::Endpoint::from_shared(url)
            .map_err(|e| Error::Controller(e.to_string()))?
            .connect()
            .await
            .map_err(|e| Error::Controller(e.to_string()))?;
        Ok(Self {
            client: Client::Plain(RunServiceClient::new(channel)),
            org,
        })
    }

    /// One page of runs matching `q`. Pass the returned token back as `token`
    /// for the next page; an empty token means there are no more.
    pub async fn list(&self, q: &RunQuery, token: &str) -> Result<(Vec<RunInfo>, String), Error> {
        let req = list_request(q, &self.org, token);
        let resp = match &self.client {
            Client::Plain(c) => c.clone().list_runs(req).await,
            Client::Authenticated(c) => c.clone().list_runs(req).await,
        }
        .map_err(|s| Error::Controller(format!("ListRuns: {s}")))?
        .into_inner();
        let runs = resp.runs.into_iter().filter_map(run_info).collect();
        Ok((runs, resp.token))
    }

    /// The inputs and outputs of one action (outputs are empty until it ends).
    pub async fn action_data(&self, action: &ActionIdentifier) -> Result<(Inputs, Outputs), Error> {
        let req = GetActionDataRequest {
            action_id: Some(action.clone()),
        };
        let resp = match &self.client {
            Client::Plain(c) => c.clone().get_action_data(req).await,
            Client::Authenticated(c) => c.clone().get_action_data(req).await,
        }
        .map_err(|s| Error::Controller(format!("GetActionData: {s}")))?
        .into_inner();
        Ok((
            resp.inputs.unwrap_or_default(),
            resp.outputs.unwrap_or_default(),
        ))
    }
}

fn env_nonempty(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.is_empty())
}

fn eq(field: &str, value: impl Into<String>) -> Filter {
    Filter {
        function: filter::Function::Equal as i32,
        field: field.into(),
        values: vec![value.into()],
    }
}

/// The request `flyte.remote.Run.listall` would send for the same query.
fn list_request(q: &RunQuery, org: &str, token: &str) -> ListRunsRequest {
    let mut filters = Vec::new();
    match q.phases.as_slice() {
        [] => {}
        [one] => filters.push(eq("phase", (*one as i32).to_string())),
        many => filters.push(Filter {
            function: filter::Function::ValueIn as i32,
            field: "phase".into(),
            values: many.iter().map(|p| (*p as i32).to_string()).collect(),
        }),
    }
    if let Some(t) = &q.task_name {
        filters.push(eq("task_name", t.clone()));
    }
    for (k, v) in &q.labels {
        filters.push(eq(&format!("labels.{k}"), v.clone()));
    }
    for k in &q.label_keys {
        filters.push(Filter {
            function: filter::Function::Exists as i32,
            field: format!("labels.{k}"),
            values: vec![],
        });
    }
    let order = Sort {
        key: "created_at".into(),
        direction: if q.newest_first {
            sort::Direction::Descending
        } else {
            sort::Direction::Ascending
        } as i32,
    };
    // Both sort fields: the Python SDK still sends the deprecated `sort_by`,
    // so that is what every backend honours today; `sort_by_fields` is its
    // replacement.
    #[allow(deprecated)]
    let request = ListRequest {
        limit: q.limit,
        token: token.to_string(),
        sort_by: Some(order.clone()),
        sort_by_fields: vec![order],
        filters,
        ..Default::default()
    };
    ListRunsRequest {
        request: Some(request),
        scope_by: Some(list_runs_request::ScopeBy::ProjectId(ProjectIdentifier {
            organization: org.to_string(),
            domain: q.domain.clone(),
            name: q.project.clone(),
        })),
        ..Default::default()
    }
}

fn run_info(run: flyteidl2::flyteidl::workflow::Run) -> Option<RunInfo> {
    let action = run.action?;
    let id = action.id?;
    let status = action.status.unwrap_or_default();
    let start_time = status
        .start_time
        .map(|t| t.seconds as f64 + t.nanos as f64 / 1e9)
        .unwrap_or(0.0);
    Some(RunInfo {
        name: id
            .run
            .as_ref()
            .map(|r: &RunIdentifier| r.name.clone())
            .unwrap_or_default(),
        phase: ActionPhase::try_from(status.phase).unwrap_or(ActionPhase::Unspecified),
        start_time,
        labels: run.labels.into_iter().collect(),
        action: id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query() -> RunQuery {
        RunQuery {
            project: "p".into(),
            domain: "development".into(),
            phases: vec![ActionPhase::Succeeded],
            task_name: Some("env.task".into()),
            labels: [("agent".to_string(), "kiwi".to_string())].into(),
            label_keys: vec!["kind".into()],
            newest_first: true,
            limit: 40,
        }
    }

    #[test]
    fn filters_match_the_python_sdk_conventions() {
        let req = list_request(&query(), "demo", "");
        let lr = req.request.unwrap();
        let fs: Vec<(i32, &str, Vec<String>)> = lr
            .filters
            .iter()
            .map(|f| (f.function, f.field.as_str(), f.values.clone()))
            .collect();
        assert_eq!(
            fs,
            vec![
                (
                    filter::Function::Equal as i32,
                    "phase",
                    vec![(ActionPhase::Succeeded as i32).to_string()]
                ),
                (
                    filter::Function::Equal as i32,
                    "task_name",
                    vec!["env.task".to_string()]
                ),
                (
                    filter::Function::Equal as i32,
                    "labels.agent",
                    vec!["kiwi".to_string()]
                ),
                (filter::Function::Exists as i32, "labels.kind", vec![]),
            ]
        );
        let s = &lr.sort_by_fields[0];
        assert_eq!(
            (s.key.as_str(), s.direction),
            ("created_at", sort::Direction::Descending as i32)
        );
        #[allow(deprecated)]
        let legacy = lr.sort_by.clone();
        assert_eq!(legacy.as_ref(), Some(s));
        assert_eq!(lr.limit, 40);
        match req.scope_by.unwrap() {
            list_runs_request::ScopeBy::ProjectId(p) => {
                assert_eq!(
                    (p.organization.as_str(), p.name.as_str(), p.domain.as_str()),
                    ("demo", "p", "development")
                )
            }
            other => panic!("unexpected scope {other:?}"),
        }
    }

    #[test]
    fn several_phases_are_one_value_in_filter() {
        let mut q = query();
        q.phases = vec![ActionPhase::Succeeded, ActionPhase::Failed];
        q.task_name = None;
        q.labels.clear();
        q.label_keys.clear();
        let lr = list_request(&q, "", "tok").request.unwrap();
        assert_eq!(lr.token, "tok");
        assert_eq!(lr.filters.len(), 1);
        assert_eq!(lr.filters[0].function, filter::Function::ValueIn as i32);
        assert_eq!(lr.filters[0].values.len(), 2);
    }

    #[test]
    fn a_listed_run_flattens_to_run_info() {
        use flyteidl2::flyteidl::workflow::{Action, ActionStatus, Run};
        let run = Run {
            action: Some(Action {
                id: Some(ActionIdentifier {
                    run: Some(RunIdentifier {
                        name: "r1".into(),
                        ..Default::default()
                    }),
                    name: "a0".into(),
                }),
                status: Some(ActionStatus {
                    phase: ActionPhase::Succeeded as i32,
                    start_time: Some(flyteidl2::google::protobuf::Timestamp {
                        seconds: 10,
                        nanos: 500_000_000,
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            labels: [("agent".to_string(), "kiwi".to_string())].into(),
        };
        let info = run_info(run).unwrap();
        assert_eq!(info.name, "r1");
        assert_eq!(info.phase, ActionPhase::Succeeded);
        assert_eq!(info.start_time, 10.5);
        assert_eq!(info.labels.get("agent").map(String::as_str), Some("kiwi"));
        assert_eq!(info.action.name, "a0");
    }
}
