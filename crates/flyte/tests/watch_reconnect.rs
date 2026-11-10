//! A child's completion must reach `wait_task` even when the server ends the
//! parent's watch stream while the child is still running. flyte_core 0.1.0
//! spun on the ended stream and never reconnected, so the caller hung forever
//! (demo run ugq9mw6f6zjr99pss8w6).
//!
//! The controller watches through StateService or, with `_U_USE_ACTIONS=1`,
//! ActionsService. The fake serves both, and the test runs once per path.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use flyte::controller::{Controller, TaskRecord, TaskResult, runtime};
use flyteidl2::flyteidl::actions::{
    self as actions, AbortRequest, AbortResponse, EnqueueRequest, EnqueueResponse, SignalRequest,
    SignalResponse, UpdateRequest, UpdateResponse, WatchForUpdatesRequest, WatchForUpdatesResponse,
    actions_service_server::{ActionsService, ActionsServiceServer},
};
use flyteidl2::flyteidl::common::{ActionIdentifier, ActionPhase, RunIdentifier};
use flyteidl2::flyteidl::core::{Identifier, TaskTemplate};
use flyteidl2::flyteidl::task::TaskSpec;
use flyteidl2::flyteidl::workflow::{
    AbortQueuedActionRequest, AbortQueuedActionResponse, AbortQueuedRunRequest,
    AbortQueuedRunResponse, ActionUpdate, ControlMessage, EnqueueActionRequest,
    EnqueueActionResponse, GetRequest, GetResponse, PutRequest, PutResponse, WatchRequest,
    WatchResponse,
    queue_service_server::{QueueService, QueueServiceServer},
    state_service_server::{StateService, StateServiceServer},
    watch_request, watch_response,
};
use tokio::sync::{Notify, mpsc};
use tokio_stream::wrappers::{ReceiverStream, TcpListenerStream};
use tonic::{Request, Response, Status};

/// What a watch stream sends, before it is mapped to either service's message.
enum Event {
    Sentinel,
    Update(Box<ActionUpdate>),
}

#[derive(Default)]
struct Fake {
    enqueued: Mutex<Vec<String>>,
    enqueue_seen: Notify,
    watches: Mutex<usize>,
}

#[derive(Clone)]
struct Svc(Arc<Fake>);

impl Svc {
    fn enqueued(&self, action_id: Option<ActionIdentifier>) {
        self.0
            .enqueued
            .lock()
            .unwrap()
            .push(action_id.unwrap().name);
        self.0.enqueue_seen.notify_one();
    }

    /// The first stream sends only the sentinel and ends cleanly once the child
    /// is enqueued, so the child is still running when it ends. Later streams
    /// report every enqueued child as succeeded and stay open, as a live server
    /// would.
    fn watch<T: Send + 'static>(
        &self,
        parent: Option<ActionIdentifier>,
        wrap: fn(Event) -> T,
    ) -> ReceiverStream<Result<T, Status>> {
        let n = {
            let mut w = self.0.watches.lock().unwrap();
            *w += 1;
            *w
        };
        let (tx, rx) = mpsc::channel(8);
        let fake = self.0.clone();
        tokio::spawn(async move {
            tx.send(Ok(wrap(Event::Sentinel))).await.unwrap();
            if n == 1 {
                fake.enqueue_seen.notified().await;
                return;
            }
            let run = parent.and_then(|p| p.run);
            let enqueued = fake.enqueued.lock().unwrap().clone();
            for name in enqueued {
                let update = ActionUpdate {
                    action_id: Some(ActionIdentifier {
                        name,
                        run: run.clone(),
                    }),
                    phase: ActionPhase::Succeeded as i32,
                    output_uri: "s3://bucket/out".into(),
                    ..Default::default()
                };
                tx.send(Ok(wrap(Event::Update(Box::new(update)))))
                    .await
                    .unwrap();
            }
            tx.closed().await;
        });
        ReceiverStream::new(rx)
    }
}

#[tonic::async_trait]
impl QueueService for Svc {
    async fn enqueue_action(
        &self,
        req: Request<EnqueueActionRequest>,
    ) -> Result<Response<EnqueueActionResponse>, Status> {
        self.enqueued(req.into_inner().action_id);
        Ok(Response::new(EnqueueActionResponse::default()))
    }
    async fn abort_queued_run(
        &self,
        _: Request<AbortQueuedRunRequest>,
    ) -> Result<Response<AbortQueuedRunResponse>, Status> {
        Err(Status::unimplemented("abort_queued_run"))
    }
    async fn abort_queued_action(
        &self,
        _: Request<AbortQueuedActionRequest>,
    ) -> Result<Response<AbortQueuedActionResponse>, Status> {
        Err(Status::unimplemented("abort_queued_action"))
    }
}

#[tonic::async_trait]
impl StateService for Svc {
    async fn put(&self, _: Request<PutRequest>) -> Result<Response<PutResponse>, Status> {
        Err(Status::unimplemented("put"))
    }
    async fn get(&self, _: Request<GetRequest>) -> Result<Response<GetResponse>, Status> {
        Err(Status::unimplemented("get"))
    }
    type WatchStream = ReceiverStream<Result<WatchResponse, Status>>;
    async fn watch(
        &self,
        req: Request<WatchRequest>,
    ) -> Result<Response<Self::WatchStream>, Status> {
        let parent = match req.into_inner().filter {
            Some(watch_request::Filter::ParentActionId(a)) => Some(a),
            _ => None,
        };
        Ok(Response::new(self.watch(parent, |e| WatchResponse {
            message: Some(match e {
                Event::Sentinel => {
                    watch_response::Message::ControlMessage(ControlMessage::default())
                }
                Event::Update(u) => watch_response::Message::ActionUpdate(*u),
            }),
        })))
    }
}

#[tonic::async_trait]
impl ActionsService for Svc {
    async fn enqueue(
        &self,
        req: Request<EnqueueRequest>,
    ) -> Result<Response<EnqueueResponse>, Status> {
        self.enqueued(req.into_inner().action.and_then(|a| a.action_id));
        Ok(Response::new(EnqueueResponse::default()))
    }
    type WatchForUpdatesStream = ReceiverStream<Result<WatchForUpdatesResponse, Status>>;
    async fn watch_for_updates(
        &self,
        req: Request<WatchForUpdatesRequest>,
    ) -> Result<Response<Self::WatchForUpdatesStream>, Status> {
        let parent = match req.into_inner().filter {
            Some(actions::watch_for_updates_request::Filter::ParentActionId(a)) => Some(a),
            _ => None,
        };
        Ok(Response::new(self.watch(parent, |e| {
            use actions::watch_for_updates_response::Message;
            WatchForUpdatesResponse {
                message: Some(match e {
                    Event::Sentinel => Message::ControlMessage(ControlMessage::default()),
                    Event::Update(u) => Message::ActionUpdate(*u),
                }),
            }
        })))
    }
    async fn update(&self, _: Request<UpdateRequest>) -> Result<Response<UpdateResponse>, Status> {
        Err(Status::unimplemented("update"))
    }
    async fn abort(&self, _: Request<AbortRequest>) -> Result<Response<AbortResponse>, Status> {
        Err(Status::unimplemented("abort"))
    }
    async fn signal(&self, _: Request<SignalRequest>) -> Result<Response<SignalResponse>, Status> {
        Err(Status::unimplemented("signal"))
    }
}

/// Start a fake backend, submit one child through a fresh controller, and wait
/// for it. Returns how many watch streams the controller opened.
fn submit_and_wait_across_a_stream_end() -> usize {
    let fake = Arc::new(Fake::default());
    let svc = Svc(fake.clone());
    let listener = runtime()
        .block_on(tokio::net::TcpListener::bind("127.0.0.1:0"))
        .unwrap();
    let addr = listener.local_addr().unwrap();
    runtime().spawn(
        tonic::transport::Server::builder()
            .add_service(StateServiceServer::new(svc.clone()))
            .add_service(QueueServiceServer::new(svc.clone()))
            .add_service(ActionsServiceServer::new(svc))
            .serve_with_incoming(TcpListenerStream::new(listener)),
    );

    let run = RunIdentifier {
        org: "o".into(),
        project: "p".into(),
        domain: "d".into(),
        name: "r".into(),
    };
    let controller = Controller::without_auth(run, format!("http://{addr}"), 2).unwrap();
    let spec = TaskSpec {
        task_template: Some(TaskTemplate {
            id: Some(Identifier {
                name: "child".into(),
                ..Default::default()
            }),
            ..Default::default()
        }),
        ..Default::default()
    };

    let result = runtime().block_on(async {
        controller
            .start_task(TaskRecord {
                parent_action_name: "a0".into(),
                action_name: "child-1".into(),
                spec,
                group: None,
                inputs_uri: "s3://bucket/in".into(),
                run_output_base: "s3://bucket".into(),
                cache_key: None,
            })
            .await
            .unwrap();
        tokio::time::timeout(
            Duration::from_secs(30),
            controller.wait_task("a0", "child-1"),
        )
        .await
    });

    match result
        .expect("wait_task hung after the watch stream ended")
        .unwrap()
    {
        TaskResult::Succeeded { outputs_uri } => {
            assert_eq!(outputs_uri.as_deref(), Some("s3://bucket/out"))
        }
        TaskResult::Failed(f) => panic!("child failed: {}", f.message),
    }
    *fake.watches.lock().unwrap()
}

#[test]
fn completion_arrives_after_the_server_ends_the_watch_stream() {
    for actions_service in ["0", "1"] {
        // SAFETY: the only test in this binary, and no controller thread reads
        // the environment while it is set: it is read once, at construction.
        unsafe { std::env::set_var("_U_USE_ACTIONS", actions_service) };
        let watches = submit_and_wait_across_a_stream_end();
        assert!(
            watches >= 2,
            "_U_USE_ACTIONS={actions_service}: the informer never reconnected"
        );
    }
}
