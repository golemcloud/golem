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

use super::*;

impl<T: 'static> Debug for FilesystemCall<T> {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FilesystemCall")
            .finish_non_exhaustive()
    }
}

impl<T: Send + 'static> FilesystemCall<T> {
    pub(super) fn new(
        lease: CallLease,
        task: impl Future<Output = Result<T, Error>> + Send + 'static,
    ) -> Self {
        Self {
            state: CallState::Unstarted {
                task: Some(Box::pin(task)),
                lease: Some(lease),
            },
        }
    }
}

impl<T: Send + 'static> Future for FilesystemCall<T> {
    type Output = Result<T, Error>;

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        loop {
            match &mut self.state {
                CallState::Unstarted { task, lease } => {
                    let task = task.take().expect("filesystem call task already started");
                    let lease = lease.take().expect("filesystem call lease already started");
                    self.state = CallState::CallerDriven {
                        task,
                        lease: Some(lease),
                    };
                }
                CallState::CallerDriven { task, lease } => {
                    let result = match task.as_mut().poll(context) {
                        Poll::Ready(result) => result,
                        Poll::Pending => return Poll::Pending,
                    };
                    drop(lease.take());
                    self.state = CallState::Done;
                    return Poll::Ready(result);
                }
                CallState::Done => panic!("filesystem call polled after completion"),
            }
        }
    }
}

impl<T: 'static> Drop for FilesystemCall<T> {
    fn drop(&mut self) {
        if !matches!(self.state, CallState::CallerDriven { .. }) {
            return;
        }
        let CallState::CallerDriven { task, mut lease } =
            std::mem::replace(&mut self.state, CallState::Done)
        else {
            unreachable!()
        };
        spawn_module_task(async move {
            drop(task.await);
            drop(lease.take());
        });
    }
}

impl Future for FilesystemClose {
    type Output = Result<(), Error>;

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        loop {
            match &mut self.state {
                CloseState::Unstarted { task } => {
                    let task = task.take().expect("filesystem close task already started");
                    self.state = CloseState::CallerDriven { task: Some(task) };
                }
                CloseState::CallerDriven { task } => {
                    let result = match task
                        .as_mut()
                        .expect("filesystem close task missing")
                        .as_mut()
                        .poll(context)
                    {
                        Poll::Ready(result) => result,
                        Poll::Pending => return Poll::Pending,
                    };
                    task.take();
                    self.state = CloseState::Done;
                    return Poll::Ready(result);
                }
                CloseState::Done => panic!("filesystem close polled after completion"),
            }
        }
    }
}

impl Drop for FilesystemClose {
    fn drop(&mut self) {
        let task = match &mut self.state {
            CloseState::Unstarted { task } | CloseState::CallerDriven { task } => task.take(),
            CloseState::Done => return,
        };
        if let Some(task) = task {
            spawn_module_task(async move {
                drop(task.await);
            });
        }
    }
}

impl OpenNode {
    /// Returns whether this open node is a file or directory.
    ///
    /// Host adapters use this after `open`; the value describes the opened object in its original
    /// generation and performs no new lifecycle admission.
    pub(crate) fn kind(&self) -> ObjectKind {
        match self {
            Self::File(_) => ObjectKind::File,
            Self::Directory(_) => ObjectKind::Directory,
        }
    }

    /// Returns the access mode granted when this node was opened.
    ///
    /// Host adapters use it to enforce descriptor capabilities before submitting mutations. The
    /// mode remains available until close and does not revalidate generation access.
    pub(crate) fn access(&self) -> AccessMode {
        node_ownership(self).access
    }
}

impl Debug for File {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("File").finish_non_exhaustive()
    }
}

impl Debug for Directory {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("Directory").finish_non_exhaustive()
    }
}

impl Drop for NodeOwnership {
    fn drop(&mut self) {
        let (Some(sandbox), Some(node_lease), Some(close)) = (
            self.sandbox.take(),
            self.node_lease.take(),
            self.close.take(),
        ) else {
            return;
        };
        drop(close(sandbox, node_lease));
    }
}

/// Explicitly closes an open file or directory.
///
/// Host adapters await this when closing a descriptor; dropping an `OpenNode` starts the same work
/// in the background. The returned close completes the sandbox close and relinquishes the node
/// lease even if the caller drops it, and reports sandbox or generation invalidation errors.
pub(crate) fn close(node: OpenNode) -> FilesystemClose {
    match node {
        OpenNode::File(file) => file.ownership.into_close(),
        OpenNode::Directory(directory) => directory.ownership.into_close(),
    }
}

impl<Adapter: SandboxFilesystemAdapter> FilesystemGeneration<Adapter> {
    pub(super) async fn register_opened(
        self: Arc<Self>,
        opened: SandboxOpened,
        access: AccessMode,
    ) -> Result<Opened, Error> {
        let node = opened.into_node();
        let (node_lease, return_to_caller) = self.registry.register_node();
        let open_node = self.wrap_node(node, access, node_lease);
        if return_to_caller {
            Ok(Opened { node: open_node })
        } else {
            close(open_node).await?;
            Err(Error::Access(AccessError::Revoked))
        }
    }

    pub(super) fn wrap_node(
        self: &Arc<Self>,
        node: SandboxNode,
        access: AccessMode,
        node_lease: NodeLease,
    ) -> OpenNode {
        let generation_id = Arc::as_ptr(self) as usize;
        let generation = Arc::clone(self);
        let ownership = NodeOwnership {
            sandbox: Some(node.clone()),
            generation_id,
            access,
            node_lease: Some(node_lease),
            close: Some(Box::new(move |node, node_lease| {
                generation.start_close(node, node_lease)
            })),
        };
        match node {
            SandboxNode::File(_) => OpenNode::File(File { ownership }),
            SandboxNode::Directory(_) => OpenNode::Directory(Directory { ownership }),
        }
    }

    pub(super) fn start_close(
        self: Arc<Self>,
        node: SandboxNode,
        node_lease: NodeLease,
    ) -> FilesystemClose {
        let call_lease = self.registry.lease_internal_call();
        FilesystemClose {
            state: CloseState::Unstarted {
                task: Some(Box::pin(async move {
                    let result = execute_close(Arc::clone(&self), node).await;
                    drop(node_lease);
                    drop(call_lease);
                    result
                })),
            },
        }
    }
}

pub(super) async fn execute_close<Adapter: SandboxFilesystemAdapter>(
    generation: Arc<FilesystemGeneration<Adapter>>,
    node: SandboxNode,
) -> Result<(), Error> {
    let mut budget = RetryBudget::new(2);
    loop {
        let result = {
            let sandbox = generation.sandbox.read().await;
            let sandbox = sandbox.as_ref().ok_or(Error::RuntimeInvalidated)?;
            sandbox.close(node.clone()).await
        };
        let error = match result {
            Ok(()) => return Ok(()),
            Err(error) => error,
        };
        match decide_generation_effect(
            &generation,
            &error,
            mutation_failure_evidence(&error),
            budget,
        )
        .await
        {
            EffectDecision::RetryAfterProvenNoEffect if budget.consume() => continue,
            EffectDecision::ReturnFailure(cause) => return Err(classified_error(cause, error)),
            EffectDecision::ReclaimCapacityThenRetry => {
                return Err(classified_error(FailureCause::PhysicalCapacity, error));
            }
            _ => {
                generation.invalidate();
                return Err(Error::RuntimeInvalidated);
            }
        }
    }
}
