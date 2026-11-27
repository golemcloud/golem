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
use crate::config::{CompileWorkerConfig, RegistryServiceConfig, StaticRegistryServiceConfig};
use crate::model::*;
use golem_common::model::component::{ComponentId, ComponentRevision};
use golem_common::model::diff::Hash;
use golem_service_base::service::compiled_component::CompiledComponentService;
use std::sync::Arc;
use tokio::sync::mpsc;
use wasmtime::Engine;

#[derive(Clone)]
pub struct ComponentCompilationService {
    queue: mpsc::Sender<CompilationRequest>,
}

impl ComponentCompilationService {
    pub async fn new(
        compile_worker: CompileWorkerConfig,
        registry_service: RegistryServiceConfig,
        engine: Engine,
        compiled_component_service: Arc<dyn CompiledComponentService>,
    ) -> Self {
        let (compile_tx, compile_rx) = mpsc::channel(compile_worker.queue_capacity.get());
        let (upload_tx, upload_rx) = mpsc::channel(compile_worker.upload_queue_capacity.get());

        CompileWorker::start(
            registry_service.static_config(),
            compile_worker,
            engine.clone(),
            compiled_component_service.clone(),
            upload_tx,
            compile_rx,
        )
        .await;

        UploadWorker::start(compiled_component_service.clone(), upload_rx);

        Self { queue: compile_tx }
    }

    pub async fn enqueue_compilation(
        &self,
        component_id: ComponentId,
        component_revision: ComponentRevision,
        wasm_hash: Hash,
        sender: Option<StaticRegistryServiceConfig>,
    ) -> Result<(), CompilationError> {
        tracing::info!(
            component_id = component_id.to_string(),
            component_revision = component_revision.to_string(),
            "Enqueueing compilation for component",
        );
        let request = CompilationRequest {
            component: ComponentIdAndRevision {
                id: component_id,
                revision: component_revision,
            },
            wasm_hash,
            sender,
        };
        let permit = self.queue.try_reserve().map_err(|error| match error {
            mpsc::error::TrySendError::Full(_) => CompilationError::QueueFull,
            mpsc::error::TrySendError::Closed(_) => CompilationError::QueueClosed,
        })?;
        // Count before publishing so the receiver cannot decrement first.
        crate::metrics::increment_queue_length();
        permit.send(request);
        Ok(())
    }
}

#[cfg(test)]
test_r::sequential_suite!(tests);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grpc::CompileGrpcService;
    use async_trait::async_trait;
    use futures::FutureExt;
    use golem_api_grpc::proto::golem::componentcompilation::v1::{
        ComponentCompilationRequest, component_compilation_error, component_compilation_response,
        component_compilation_service_server::ComponentCompilationService as _,
    };
    use golem_common::base_model::api::error_code::INTERNAL_COMPILATION_QUEUE_FULL;
    use golem_common::model::environment::EnvironmentId;
    use golem_service_base::error::worker_executor::WorkerExecutorError;
    use golem_service_base::service::compiled_component::DefaultCompiledComponentService;
    use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;
    use test_r::{test, timeout};
    use tokio::sync::Semaphore;
    use wasmtime::component::Component;

    fn queue_length(name: &str) -> f64 {
        crate::metrics::register_all()
            .gather()
            .into_iter()
            .find(|metric| metric.name() == name)
            .unwrap()
            .get_metric()[0]
            .get_gauge()
            .as_ref()
            .unwrap()
            .value()
    }

    #[test]
    #[timeout("30s")]
    async fn compilation_rejects_mismatched_wasm_and_retries_only_missing_artifacts() {
        let engine =
            Engine::new(&golem_common::wasmtime_config::create_wasmtime_config_without_fs_cache())
                .unwrap();
        let storage = Arc::new(DefaultCompiledComponentService::new(Arc::new(
            InMemoryBlobStorage::new(),
        )));
        let (queue, requests) = mpsc::channel(2);
        let (upload_queue, mut artifacts) = mpsc::channel(1);
        let wasm = b"\0asm\x0d\0\x01\0";
        let hash = Hash::new(blake3::hash(wasm));
        let compile_count = Arc::new(AtomicUsize::new(0));
        CompileWorker::start_queue(engine.clone(), storage.clone(), upload_queue, requests, {
            let engine = engine.clone();
            let compile_count = compile_count.clone();
            move |request| {
                compile_count.fetch_add(1, Ordering::Relaxed);
                let engine = engine.clone();
                async move {
                    let mut bytes = wasm.to_vec();
                    if request.component.revision == ComponentRevision::INITIAL {
                        bytes.extend_from_slice(b"\0\x02\x01x"); // A valid custom section named x.
                    }
                    CompileWorker::compile_downloaded(engine, &request, bytes).await
                }
            }
        });
        let service = ComponentCompilationService { queue };
        let id = ComponentId::new();
        let enqueue = async |revision| {
            service
                .enqueue_compilation(id, ComponentRevision::new(revision).unwrap(), hash, None)
                .await
                .unwrap();
        };

        enqueue(u64::from(ComponentRevision::INITIAL)).await;
        enqueue(1).await;
        let failed = artifacts.recv().await.unwrap();
        assert_eq!(u64::from(failed.component_and_revision.revision), 1);
        // Model an upload that has completed without writing the artifact.
        crate::metrics::decrement_upload_queue_length();
        failed.upload_finished.send(()).unwrap();
        assert!(!storage.exists(hash, &engine).await.unwrap());

        enqueue(2).await;
        let retried = artifacts.recv().await.unwrap();
        assert_eq!(u64::from(retried.component_and_revision.revision), 2);
        crate::metrics::decrement_upload_queue_length();
        storage
            .put(
                hash,
                id,
                retried.component_and_revision.revision,
                &retried.component,
            )
            .await
            .unwrap();
        retried.upload_finished.send(()).unwrap();

        enqueue(3).await;
        drop(service);
        assert!(artifacts.recv().await.is_none());
        assert_eq!(compile_count.load(Ordering::Relaxed), 3);
    }

    #[test]
    async fn enqueue_rejects_full_and_closed_queues_without_waiting() {
        let (queue, mut receiver) = mpsc::channel(1);
        let service = CompileGrpcService::new(
            Arc::new(ComponentCompilationService { queue }),
            RegistryServiceConfig::default(),
        );
        let component_id = ComponentId::new();
        let environment_id = EnvironmentId::new();
        let request = ComponentCompilationRequest {
            component_id: Some(component_id.into()),
            component_revision: 7,
            component_service_port: None,
            environment_id: Some(environment_id.into()),
            wasm_hash: Some(Hash::empty().into()),
        };
        let accepted = service
            .enqueue_compilation(tonic::Request::new(request.clone()))
            .now_or_never()
            .expect("admission must not wait")
            .unwrap()
            .into_inner();
        assert!(matches!(
            accepted.result,
            Some(component_compilation_response::Result::Success(_))
        ));
        assert_eq!(
            queue_length("golem_component_compilation_queue_length"),
            1.0
        );

        let rejected = service
            .enqueue_compilation(tonic::Request::new(request.clone()))
            .now_or_never()
            .expect("a full queue must not wait")
            .unwrap()
            .into_inner();
        let Some(component_compilation_response::Result::Failure(error)) = rejected.result else {
            panic!("full queue must reject admission");
        };
        assert!(
            matches!(error.error, Some(component_compilation_error::Error::InternalError(body))
            if body.code == INTERNAL_COMPILATION_QUEUE_FULL)
        );
        assert_eq!(
            queue_length("golem_component_compilation_queue_length"),
            1.0
        );
        let queued = receiver.recv().await.unwrap();
        crate::metrics::decrement_queue_length();
        assert_eq!(queued.component.id, component_id);
        assert_eq!(u64::from(queued.component.revision), 7);
        assert!(
            receiver.try_recv().is_err(),
            "rejected request must not enter the queue"
        );
        drop(receiver);

        let closed = service
            .enqueue_compilation(tonic::Request::new(request))
            .now_or_never()
            .expect("a closed queue must not wait")
            .unwrap()
            .into_inner();
        let Some(component_compilation_response::Result::Failure(error)) = closed.result else {
            panic!("closed queue must fail admission");
        };
        assert!(
            matches!(error.error, Some(component_compilation_error::Error::InternalError(body))
            if body.code != INTERNAL_COMPILATION_QUEUE_FULL && body.error.contains("closed"))
        );
        assert_eq!(
            queue_length("golem_component_compilation_queue_length"),
            0.0
        );
    }

    struct GatedStorage {
        inner: DefaultCompiledComponentService,
        gets: Semaphore,
        get_gate: Semaphore,
        uploads: mpsc::UnboundedSender<ComponentRevision>,
        upload_gate: Semaphore,
    }

    #[async_trait]
    impl CompiledComponentService for GatedStorage {
        async fn exists(&self, hash: Hash, engine: &Engine) -> anyhow::Result<bool> {
            self.gets.add_permits(1);
            self.get_gate.acquire().await.unwrap().forget();
            self.inner.exists(hash, engine).await
        }

        async fn get(
            &self,
            _: Hash,
            _: ComponentId,
            _: ComponentRevision,
            _: &Engine,
        ) -> Result<Option<Component>, WorkerExecutorError> {
            panic!("precompilation must check existence without loading an artifact")
        }

        async fn put(
            &self,
            hash: Hash,
            id: ComponentId,
            revision: ComponentRevision,
            component: &Component,
        ) -> Result<(), WorkerExecutorError> {
            self.uploads.send(revision).unwrap();
            self.upload_gate.acquire().await.unwrap().forget();
            self.inner.put(hash, id, revision, component).await
        }
    }

    #[test]
    #[timeout("30s")]
    async fn configured_queues_bound_pending_requests_and_compiled_artifacts() {
        let engine =
            Engine::new(&golem_common::wasmtime_config::create_wasmtime_config_without_fs_cache())
                .unwrap();
        let (uploads, mut uploaded) = mpsc::unbounded_channel();
        let storage = Arc::new(GatedStorage {
            inner: DefaultCompiledComponentService::new(Arc::new(InMemoryBlobStorage::new())),
            gets: Semaphore::new(0),
            get_gate: Semaphore::new(0),
            uploads,
            upload_gate: Semaphore::new(0),
        });
        let (queue, requests) = mpsc::channel(2);
        let (upload_queue, artifacts) = mpsc::channel(1);
        let artifact = Component::new(&engine, "(component)").unwrap();
        CompileWorker::start_queue(engine, storage.clone(), upload_queue, requests, move |_| {
            std::future::ready(Ok(artifact.clone()))
        });
        UploadWorker::start(storage.clone(), artifacts);
        let service = ComponentCompilationService { queue };
        let id = ComponentId::new();
        let hash = |revision: u8| Hash::new(blake3::hash(&[revision]));
        service
            .enqueue_compilation(id, ComponentRevision::INITIAL, hash(0), None)
            .await
            .unwrap();
        storage.gets.acquire().await.unwrap().forget();
        for revision in [1, 2] {
            service
                .enqueue_compilation(
                    id,
                    ComponentRevision::new(revision).unwrap(),
                    hash(revision as u8),
                    None,
                )
                .await
                .unwrap();
        }
        assert!(matches!(
            service
                .enqueue_compilation(id, ComponentRevision::new(3).unwrap(), hash(3), None)
                .await,
            Err(CompilationError::QueueFull)
        ));
        storage.get_gate.add_permits(10);
        assert_eq!(u64::from(uploaded.recv().await.unwrap()), 0);
        storage.gets.acquire_many(2).await.unwrap().forget();
        // One upload is active, one is queued, and the compiler holds the third artifact.
        for revision in [3, 4] {
            service
                .enqueue_compilation(
                    id,
                    ComponentRevision::new(revision).unwrap(),
                    hash(revision as u8),
                    None,
                )
                .await
                .unwrap();
        }
        assert!(matches!(
            service
                .enqueue_compilation(id, ComponentRevision::new(5).unwrap(), hash(5), None)
                .await,
            Err(CompilationError::QueueFull)
        ));
        assert!(
            tokio::time::timeout(Duration::from_millis(50), storage.gets.acquire())
                .await
                .is_err(),
            "a full upload queue must stop the compiler from starting another component"
        );
        assert_eq!(
            queue_length("golem_component_compilation_queue_length"),
            2.0
        );
        assert_eq!(
            queue_length("golem_component_compilation_upload_queue_length"),
            1.0
        );
        storage.upload_gate.add_permits(4);
        for revision in 1..5 {
            assert_eq!(u64::from(uploaded.recv().await.unwrap()), revision);
        }
        storage.gets.acquire_many(2).await.unwrap().forget();
        service
            .enqueue_compilation(
                ComponentId::new(),
                ComponentRevision::new(5).unwrap(),
                hash(4),
                None,
            )
            .await
            .unwrap();
        service
            .enqueue_compilation(id, ComponentRevision::new(6).unwrap(), hash(6), None)
            .await
            .unwrap();
        // The next distinct artifact must start while revision 4's upload is still blocked.
        storage.gets.acquire().await.unwrap().forget();
        storage.upload_gate.add_permits(2);
        assert_eq!(u64::from(uploaded.recv().await.unwrap()), 6);
        drop(service);
        drop(storage);
        assert!(
            uploaded.recv().await.is_none(),
            "the duplicate Wasm of another component must not compile or upload again"
        );
        assert_eq!(
            queue_length("golem_component_compilation_queue_length"),
            0.0
        );
        assert_eq!(
            queue_length("golem_component_compilation_upload_queue_length"),
            0.0
        );
    }
}
