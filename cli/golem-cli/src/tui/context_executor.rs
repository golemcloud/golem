// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::auth::{AuthPresenter, with_auth_presenter};
use crate::context::Context;
use crate::log::LogContext;
use std::future::Future;
use std::sync::Arc;
use std::sync::RwLock;
use tokio::runtime::Handle;
use tokio::sync::mpsc::Sender;

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct TuiContextId(u64);

#[allow(dead_code)]
impl TuiContextId {
    pub(crate) fn new(value: u64) -> Self {
        Self(value)
    }

    fn value(self) -> u64 {
        self.0
    }
}

#[allow(dead_code)]
#[derive(Clone)]
pub(crate) struct TuiLaunchContext {
    id: TuiContextId,
    context: Arc<Context>,
}

#[allow(dead_code)]
impl TuiLaunchContext {
    pub(crate) fn id(&self) -> TuiContextId {
        self.id
    }

    pub(crate) fn context(&self) -> &Arc<Context> {
        &self.context
    }

    pub(crate) fn into_context(self) -> Arc<Context> {
        self.context
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) struct TuiContextTaskResult<T> {
    context_id: TuiContextId,
    result: Result<T, String>,
    logs: Vec<String>,
}

#[allow(dead_code)]
impl<T> TuiContextTaskResult<T> {
    pub(crate) fn new(
        context_id: TuiContextId,
        result: Result<T, String>,
        logs: Vec<String>,
    ) -> Self {
        Self {
            context_id,
            result,
            logs,
        }
    }

    pub(crate) fn into_parts(self) -> (TuiContextId, Result<T, String>, Vec<String>) {
        (self.context_id, self.result, self.logs)
    }
}

#[allow(dead_code)]
pub(crate) struct TuiContextExecutor {
    runtime: Handle,
    selected: RwLock<TuiLaunchContext>,
}

impl TuiContextExecutor {
    pub(crate) fn new(initial_context: Arc<Context>) -> Self {
        Self::with_handle(initial_context, Handle::current())
    }

    #[allow(dead_code)]
    fn with_handle(initial_context: Arc<Context>, runtime: Handle) -> Self {
        Self {
            runtime,
            selected: RwLock::new(TuiLaunchContext {
                id: TuiContextId(1),
                context: initial_context,
            }),
        }
    }

    #[allow(dead_code)]
    pub(crate) fn current_context_id(&self) -> TuiContextId {
        self.launch_context().id
    }

    pub(crate) fn select_context(&self, context: Arc<Context>) -> TuiContextId {
        let mut selected = self.selected.write().unwrap();
        let id = TuiContextId(selected.id.value().saturating_add(1));
        *selected = TuiLaunchContext { id, context };
        id
    }

    #[allow(dead_code)]
    pub(crate) fn spawn<F, Fut, T, E, M>(
        &self,
        event_tx: Sender<E>,
        auth_presenter: Option<Arc<dyn AuthPresenter>>,
        work: F,
        map_event: M,
    ) -> tokio::task::JoinHandle<()>
    where
        F: FnOnce(TuiLaunchContext) -> Fut + Send + 'static,
        Fut: Future<Output = anyhow::Result<T>> + Send + 'static,
        T: Send + 'static,
        E: Send + 'static,
        M: FnOnce(TuiContextTaskResult<T>) -> E + Send + 'static,
    {
        let launch_context = self.launch_context();
        self.runtime.spawn(async move {
            let log_context = LogContext::captured();
            let context_id = launch_context.id();
            let work = log_context.scope(async move {
                work(launch_context)
                    .await
                    .map_err(|error| format!("{error:#}"))
            });
            let result = match auth_presenter {
                Some(auth_presenter) => with_auth_presenter(auth_presenter, work).await,
                None => work.await,
            };
            let logs = log_context.take_buffered_lines();
            let _ = event_tx
                .send(map_event(TuiContextTaskResult {
                    context_id,
                    result,
                    logs,
                }))
                .await;
        })
    }

    fn launch_context(&self) -> TuiLaunchContext {
        self.selected.read().unwrap().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::{TuiContextExecutor, TuiContextId};
    use crate::command::GolemCliCommand;
    use crate::context::Context;
    use crate::log::{LogIndent, Output, logln};
    use anyhow::anyhow;
    use clap::Parser;
    use std::sync::Arc;
    use tempfile::TempDir;
    use test_r::test;
    use tokio::sync::mpsc;

    #[derive(Debug, PartialEq, Eq)]
    struct TestEvent<T>(super::TuiContextTaskResult<T>);

    async fn test_context() -> (Arc<Context>, TempDir) {
        let config_dir = TempDir::new().expect("config dir");
        let command = GolemCliCommand::parse_from([
            "golem-cli",
            "--config-dir",
            config_dir.path().to_str().expect("utf-8 path"),
            "--local",
            "--disable-app-manifest-discovery",
            "tui",
        ]);
        let context = Context::new(command.global_flags, Some(Output::None))
            .await
            .expect("context");
        (Arc::new(context), config_dir)
    }

    async fn executor() -> (TuiContextExecutor, Arc<Context>, TempDir) {
        let (context, config_dir) = test_context().await;
        (
            TuiContextExecutor::with_handle(context.clone(), tokio::runtime::Handle::current()),
            context,
            config_dir,
        )
    }

    #[test]
    async fn spawned_work_receives_initial_launch_context() {
        let (executor, context, _config_dir) = executor().await;
        let (tx, mut rx) = mpsc::channel(16);

        let handle = executor.spawn(
            tx,
            None,
            |launch_context| async move {
                assert_eq!(launch_context.id().value(), 1);
                Ok((
                    launch_context.id().value(),
                    Arc::as_ptr(launch_context.context()) as usize,
                ))
            },
            TestEvent,
        );

        let event = rx.recv().await.expect("event");
        handle.abort();

        assert_eq!(event.0.context_id, TuiContextId(1));
        assert_eq!(event.0.result, Ok((1, Arc::as_ptr(&context) as usize)));
        assert!(event.0.logs.is_empty());
    }

    #[test]
    async fn completion_event_includes_captured_logs() {
        let (executor, _context, _config_dir) = executor().await;
        let (tx, mut rx) = mpsc::channel(16);

        let handle = executor.spawn(
            tx,
            None,
            |_launch_context| async move {
                logln("starting");
                let _indent = LogIndent::prefix("> ");
                logln("inside");
                Ok(42)
            },
            TestEvent,
        );

        let event = rx.recv().await.expect("event");
        handle.abort();

        assert_eq!(event.0.result, Ok(42));
        assert_eq!(event.0.logs, ["starting", "> inside"]);
    }

    #[test]
    async fn concurrent_requests_get_separate_log_buffers() {
        let (executor, _context, _config_dir) = executor().await;
        let (tx, mut rx) = mpsc::channel(16);

        let first = executor.spawn(
            tx.clone(),
            None,
            |_launch_context| async move {
                logln("first");
                tokio::task::yield_now().await;
                logln("first done");
                Ok("first")
            },
            TestEvent,
        );
        let second = executor.spawn(
            tx,
            None,
            |_launch_context| async move {
                logln("second");
                tokio::task::yield_now().await;
                logln("second done");
                Ok("second")
            },
            TestEvent,
        );

        let mut events = vec![
            rx.recv().await.expect("first event"),
            rx.recv().await.expect("second event"),
        ];
        first.abort();
        second.abort();
        events.sort_by_key(|event| event.0.result.clone().unwrap());

        assert_eq!(events[0].0.result, Ok("first"));
        assert_eq!(events[0].0.logs, ["first", "first done"]);
        assert_eq!(events[1].0.result, Ok("second"));
        assert_eq!(events[1].0.logs, ["second", "second done"]);
    }

    #[test]
    async fn failed_work_is_delivered_as_error_result() {
        let (executor, _context, _config_dir) = executor().await;
        let (tx, mut rx) = mpsc::channel(16);

        let handle = executor.spawn(
            tx,
            None,
            |_launch_context| async move {
                logln("before failure");
                Err::<(), _>(anyhow!("broken"))
            },
            TestEvent,
        );

        let event = rx.recv().await.expect("event");
        handle.abort();

        assert_eq!(event.0.result, Err("broken".to_string()));
        assert_eq!(event.0.logs, ["before failure"]);
    }

    #[test]
    async fn requests_reuse_executor_and_initial_context_id() {
        let (executor, context, _config_dir) = executor().await;
        let (tx, mut rx) = mpsc::channel(16);

        let first = executor.spawn(
            tx.clone(),
            None,
            |launch_context| async move { Ok(format!("id:{}", launch_context.id().value())) },
            TestEvent,
        );
        let second = executor.spawn(
            tx,
            None,
            |launch_context| async move {
                Ok(format!(
                    "context:{:p}",
                    Arc::as_ptr(&launch_context.into_context())
                ))
            },
            TestEvent,
        );

        let events = [
            rx.recv().await.expect("first event"),
            rx.recv().await.expect("second event"),
        ];
        first.abort();
        second.abort();

        assert_eq!(executor.current_context_id(), TuiContextId(1));
        assert!(
            events
                .iter()
                .all(|event| event.0.context_id == TuiContextId(1))
        );
        assert!(
            events
                .iter()
                .any(|event| event.0.result == Ok("id:1".to_string()))
        );
        assert!(
            events
                .iter()
                .any(|event| event.0.result == Ok(format!("context:{:p}", Arc::as_ptr(&context))))
        );
    }

    #[test]
    async fn selecting_context_increments_context_id_for_future_requests() {
        let (executor, context, _config_dir) = executor().await;

        assert_eq!(executor.current_context_id(), TuiContextId(1));
        assert_eq!(executor.select_context(context), TuiContextId(2));
        assert_eq!(executor.current_context_id(), TuiContextId(2));

        let (tx, mut rx) = mpsc::channel(16);
        let handle = executor.spawn(
            tx,
            None,
            |launch_context| async move { Ok(launch_context.id().value()) },
            TestEvent,
        );

        let event = rx.recv().await.expect("event");
        handle.abort();

        assert_eq!(event.0.context_id, TuiContextId(2));
        assert_eq!(event.0.result, Ok(2));
    }
}
