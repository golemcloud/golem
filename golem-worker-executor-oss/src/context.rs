use std::string::FromUtf8Error;
use std::sync::{Arc, RwLock};

use anyhow::Error;
use async_trait::async_trait;
use cap_std::ambient_authority;
use golem_common::model::{
    AccountId, CallingConvention, InvocationKey, VersionedWorkerId, WorkerId, WorkerMetadata,
    WorkerStatus,
};
use golem_common::proto::golem::Val;
use golem_worker_executor_base::error::{is_interrupt, GolemError};
use golem_worker_executor_base::host::managed_stdio::ManagedStandardIo;
use golem_worker_executor_base::model::{
    CurrentResourceLimits, ExecutionStatus, InterruptKind, WorkerConfig,
};
use golem_worker_executor_base::services::active_workers::ActiveWorkers;
use golem_worker_executor_base::services::blob_store::BlobStoreService;
use golem_worker_executor_base::services::golem_config::GolemConfig;
use golem_worker_executor_base::services::invocation_key::InvocationKeyService;
use golem_worker_executor_base::services::key_value::KeyValueService;
use golem_worker_executor_base::services::promise::PromiseService;
use golem_worker_executor_base::services::worker::WorkerService;
use golem_worker_executor_base::services::worker_event::WorkerEventService;
use golem_worker_executor_base::services::{HasAll, HasExtraDeps};
use golem_worker_executor_base::workerctx::{
    ExternalOperations, FuelManagement, InvocationHooks, InvocationManagement, IoCapturing,
    PublicWorkerIo, StatusManagement, WorkerCtx,
};
use tempfile::TempDir;
use tokio::runtime::Handle;
use tonic::codegen::Bytes;
use tracing::debug;
use wasmtime::component::{Instance, Linker};
use wasmtime::{AsContextMut, Engine, ResourceLimiterAsync};
use wasmtime_wasi::preview2::{
    stderr, DirPerms, FilePerms, I32Exit, Table, WasiCtx, WasiCtxBuilder, WasiView,
};
use wasmtime_wasi_http::{WasiHttpCtx, WasiHttpView};

use crate::host::{ManagedStdErr, ManagedStdIn, ManagedStdOut};
use crate::services::config::AdditionalGolemConfig;
use crate::services::{AdditionalDeps, HasAdditionalGolemConfig};

pub struct Context {
    active_workers: Arc<ActiveWorkers<Context>>,
    additional_golem_config: Arc<AdditionalGolemConfig>,
    blob_store_service: Arc<dyn BlobStoreService + Send + Sync>,
    invocation_key_service: Arc<dyn InvocationKeyService + Send + Sync>,
    key_value_service: Arc<dyn KeyValueService + Send + Sync>,
    promise_service: Arc<dyn PromiseService + Send + Sync>,
    worker_service: Arc<dyn WorkerService + Send + Sync>,

    account_id: AccountId,
    worker_id: VersionedWorkerId,
    current_invocation_key: Option<InvocationKey>,
    execution_status: Arc<RwLock<ExecutionStatus>>,

    table: Table,
    wasi: WasiCtx,
    wasi_http: WasiHttpCtx,

    promise_idx: i32,

    public_state: PublicState,
    #[allow(unused)]
    temp_dir: Arc<TempDir>,
}

#[derive(Clone)]
pub struct PublicState {
    pub event_service: Arc<dyn WorkerEventService + Send + Sync>,
    pub managed_stdio: ManagedStandardIo,
}

impl Context {
    pub fn account_id(&self) -> &AccountId {
        &self.account_id
    }

    pub fn additional_golem_config(&self) -> &Arc<AdditionalGolemConfig> {
        &self.additional_golem_config
    }

    pub fn blob_store_service(&self) -> &Arc<dyn BlobStoreService + Send + Sync> {
        &self.blob_store_service
    }

    pub fn key_value_service(&self) -> &Arc<dyn KeyValueService + Send + Sync> {
        &self.key_value_service
    }

    pub fn next_promise_id(&mut self) -> i32 {
        let promise_idx = self.promise_idx;
        self.promise_idx += 1;
        promise_idx
    }

    pub fn promise_service(&self) -> &Arc<dyn PromiseService + Send + Sync> {
        &self.promise_service
    }

    pub fn table(&self) -> &Table {
        &self.table
    }

    pub fn table_mut(&mut self) -> &mut Table {
        &mut self.table
    }
}

#[async_trait]
impl FuelManagement for Context {
    fn is_out_of_fuel(&self, _current_level: i64) -> bool {
        false
    }

    async fn borrow_fuel(&mut self) -> Result<(), GolemError> {
        Ok(())
    }

    fn borrow_fuel_sync(&mut self) {}

    async fn return_fuel(&mut self, _current_level: i64) -> Result<i64, GolemError> {
        Ok(0)
    }
}

#[async_trait]
impl InvocationManagement for Context {
    async fn set_current_invocation_key(&mut self, invocation_key: Option<InvocationKey>) {
        self.current_invocation_key = invocation_key;
    }

    async fn get_current_invocation_key(&self) -> Option<InvocationKey> {
        self.current_invocation_key.clone()
    }

    async fn interrupt_invocation_key(&mut self, key: &InvocationKey) {
        self.invocation_key_service
            .interrupt_key(&self.worker_id.worker_id, key)
    }

    async fn resume_invocation_key(&mut self, key: &InvocationKey) {
        self.invocation_key_service
            .resume_key(&self.worker_id.worker_id, key)
    }

    async fn confirm_invocation_key(
        &mut self,
        key: &InvocationKey,
        vals: Result<Vec<Val>, GolemError>,
    ) {
        self.invocation_key_service
            .confirm_key(&self.worker_id.worker_id, key, vals)
    }
}

#[async_trait]
impl IoCapturing for Context {
    async fn start_capturing_stdout(&mut self, provided_stdin: String) {
        self.public_state
            .managed_stdio
            .start_single_stdio_call(provided_stdin)
            .await
    }

    async fn finish_capturing_stdout(&mut self) -> Result<String, FromUtf8Error> {
        self.public_state
            .managed_stdio
            .finish_single_stdio_call()
            .await
    }
}

#[async_trait]
impl StatusManagement for Context {
    fn check_interrupt(&self) -> Option<InterruptKind> {
        let execution_status = self.execution_status.read().unwrap().clone();
        match execution_status {
            ExecutionStatus::Interrupting { interrupt_kind, .. } => Some(interrupt_kind),
            ExecutionStatus::Interrupted { interrupt_kind } => Some(interrupt_kind),
            _ => None,
        }
    }

    fn set_suspended(&self) {
        let mut execution_status = self.execution_status.write().unwrap();
        let current_execution_status = execution_status.clone();
        match current_execution_status {
            ExecutionStatus::Running => {
                *execution_status = ExecutionStatus::Suspended;
            }
            ExecutionStatus::Suspended => {}
            ExecutionStatus::Interrupting {
                interrupt_kind,
                await_interruption,
            } => {
                *execution_status = ExecutionStatus::Interrupted { interrupt_kind };
                await_interruption.send(()).ok();
            }
            ExecutionStatus::Interrupted { .. } => {}
        }
    }

    fn set_running(&self) {
        let mut execution_status = self.execution_status.write().unwrap();
        let current_execution_status = execution_status.clone();
        match current_execution_status {
            ExecutionStatus::Running => {}
            ExecutionStatus::Suspended => {
                *execution_status = ExecutionStatus::Running;
            }
            ExecutionStatus::Interrupting { .. } => {}
            ExecutionStatus::Interrupted { .. } => {}
        }
    }

    async fn get_worker_status(&self) -> WorkerStatus {
        match self.worker_service.get(&self.worker_id.worker_id).await {
            Some(metadata) => metadata.last_known_status.status,
            None => WorkerStatus::Idle,
        }
    }

    async fn store_worker_status(&self, status: WorkerStatus) {
        self.worker_service
            .update_status(&self.worker_id.worker_id, status, 0)
            .await
    }

    async fn deactivate(&self) {
        debug!("deactivating worker {}", self.worker_id);
        self.active_workers.remove(&self.worker_id.worker_id);
    }
}

#[async_trait]
impl InvocationHooks for Context {
    async fn on_exported_function_invoked(
        &mut self,
        _full_function_name: &str,
        _function_input: &Vec<Val>,
        _calling_convention: Option<&CallingConvention>,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    async fn on_invocation_failure(&mut self, _error: &Error) -> Result<(), Error> {
        Ok(())
    }

    async fn on_invocation_failure_deactivated(
        &mut self,
        error: &Error,
    ) -> Result<WorkerStatus, Error> {
        if is_interrupt(error) {
            Ok(WorkerStatus::Interrupted)
        } else {
            Ok(WorkerStatus::Failed)
        }
    }

    async fn on_invocation_success(
        &mut self,
        _full_function_name: &str,
        _function_input: &Vec<Val>,
        _consumed_fuel: i64,
        output: Vec<Val>,
    ) -> Result<Option<Vec<Val>>, Error> {
        Ok(Some(output))
    }
}

#[async_trait]
impl ExternalOperations<Self> for Context {
    type ExtraDeps = AdditionalDeps;

    async fn set_worker_status<T: HasAll<Self> + Send + Sync>(
        this: &T,
        worker_id: &WorkerId,
        status: WorkerStatus,
    ) {
        this.worker_service()
            .update_status(worker_id, status, 0)
            .await;
    }

    async fn get_worker_retry_count<T: HasAll<Self> + Send + Sync>(
        _this: &T,
        _worker_id: &WorkerId,
    ) -> u32 {
        0
    }

    async fn get_assumed_worker_status<T: HasAll<Self> + Send + Sync>(
        _this: &T,
        _worker_id: &WorkerId,
        metadata: &Option<WorkerMetadata>,
    ) -> WorkerStatus {
        match metadata {
            Some(metadata) => metadata.last_known_status.status.clone(),
            None => WorkerStatus::Idle,
        }
    }

    async fn prepare_instance(
        _worker_id: &VersionedWorkerId,
        _instance: &Instance,
        _store: &mut (impl AsContextMut<Data=Self> + Send),
    ) -> Result<(), GolemError> {
        Ok(())
    }

    async fn record_last_known_limits<T: HasExtraDeps<Self> + Send + Sync>(
        _this: &T,
        _account_id: &AccountId,
        _last_known_limits: &CurrentResourceLimits,
    ) -> Result<(), GolemError> {
        Ok(())
    }

    async fn on_worker_deleted<T: HasExtraDeps<Self> + Send + Sync>(
        _this: &T,
        _worker_id: &WorkerId,
    ) -> Result<(), GolemError> {
        Ok(())
    }

    async fn on_shard_assignment_changed<T: HasAll<Self> + Send + Sync>(
        _this: &T,
    ) -> Result<(), Error> {
        Ok(())
    }
}

#[async_trait]
impl WorkerCtx for Context {
    type PublicState = PublicState;

    async fn create(
        worker_id: VersionedWorkerId,
        account_id: AccountId,
        promise_service: Arc<dyn PromiseService + Send + Sync>,
        invocation_key_service: Arc<dyn InvocationKeyService + Send + Sync>,
        worker_service: Arc<dyn WorkerService + Send + Sync>,
        key_value_service: Arc<dyn KeyValueService + Send + Sync>,
        blob_store_service: Arc<dyn BlobStoreService + Send + Sync>,
        event_service: Arc<dyn WorkerEventService + Send + Sync>,
        active_workers: Arc<ActiveWorkers<Self>>,
        extra_deps: Self::ExtraDeps,
        _config: Arc<GolemConfig>,
        worker_config: WorkerConfig,
        execution_status: Arc<RwLock<ExecutionStatus>>,
        runtime: Handle,
    ) -> Result<Self, GolemError> {
        let stdio =
            ManagedStandardIo::new(worker_id.worker_id.clone(), invocation_key_service.clone());
        let stdin = ManagedStdIn::from_standard_io(runtime.clone(), stdio.clone());
        let stdout = ManagedStdOut::from_standard_io(runtime.clone(), stdio.clone(), event_service.clone());
        let stderr = ManagedStdErr::from_stderr(runtime.clone(), stderr(), event_service.clone());

        let temp_dir = Arc::new(tempfile::Builder::new().prefix("golem").tempdir().map_err(
            |e| GolemError::runtime(format!("Failed to create temporary directory: {e}")),
        )?);
        let root_dir = cap_std::fs::Dir::open_ambient_dir(temp_dir.path(), ambient_authority())
            .map_err(|e| GolemError::runtime(format!("Failed to open temporary directory: {e}")))?;

        let table = Table::new();
        let wasi = WasiCtxBuilder::new()
            .args(&worker_config.args)
            .envs(&worker_config.env)
            .stdin(stdin)
            .stdout(stdout)
            .stderr(stderr)
            .preopened_dir(
                root_dir
                    .try_clone()
                    .expect("Failed to clone root directory handle"),
                DirPerms::all(),
                FilePerms::all(),
                "/",
            )
            .preopened_dir(root_dir, DirPerms::all(), FilePerms::all(), ".")
            .build();
        let wasi_http = WasiHttpCtx;

        Ok(Context {
            active_workers,
            additional_golem_config: extra_deps.additional_golem_config(),
            blob_store_service,
            invocation_key_service,
            key_value_service,
            promise_service,
            worker_service,
            account_id,
            worker_id,
            current_invocation_key: None,
            execution_status,
            promise_idx: 0,
            table,
            wasi,
            wasi_http,
            public_state: PublicState {
                event_service,
                managed_stdio: stdio,
            },
            temp_dir,
        })
    }

    fn get_public_state(&self) -> &Self::PublicState {
        &self.public_state
    }

    fn resource_limiter(&mut self) -> &mut dyn ResourceLimiterAsync {
        self
    }

    fn worker_id(&self) -> &VersionedWorkerId {
        &self.worker_id
    }

    fn is_exit(error: &Error) -> Option<i32> {
        error
            .root_cause()
            .downcast_ref::<I32Exit>()
            .map(|exit| exit.0)
    }
}

#[async_trait]
impl ResourceLimiterAsync for Context {
    async fn memory_growing(
        &mut self,
        _current: usize,
        _desired: usize,
        _maximum: Option<usize>,
    ) -> anyhow::Result<bool> {
        Ok(true)
    }

    async fn table_growing(
        &mut self,
        _current: u32,
        _desired: u32,
        _maximum: Option<u32>,
    ) -> anyhow::Result<bool> {
        Ok(true)
    }
}

impl WasiView for Context {
    fn table(&self) -> &Table {
        &self.table
    }

    fn table_mut(&mut self) -> &mut Table {
        &mut self.table
    }

    fn ctx(&self) -> &WasiCtx {
        &self.wasi
    }

    fn ctx_mut(&mut self) -> &mut WasiCtx {
        &mut self.wasi
    }
}

impl WasiHttpView for Context {
    fn ctx(&mut self) -> &mut WasiHttpCtx {
        &mut self.wasi_http
    }

    fn table(&mut self) -> &mut Table {
        &mut self.table
    }
}

#[async_trait]
impl PublicWorkerIo for PublicState {
    fn event_service(&self) -> Arc<dyn WorkerEventService + Send + Sync> {
        self.event_service.clone()
    }

    async fn enqueue(&self, message: Bytes, invocation_key: InvocationKey) {
        self.managed_stdio.enqueue(message, invocation_key).await
    }
}

pub fn create_linker(engine: &Engine) -> wasmtime::Result<Linker<Context>> {
    let mut linker = Linker::new(engine);

    wasmtime_wasi::preview2::bindings::cli::environment::add_to_linker(&mut linker, |x| x)?;
    wasmtime_wasi::preview2::bindings::cli::exit::add_to_linker(&mut linker, |x| x)?;
    wasmtime_wasi::preview2::bindings::cli::stderr::add_to_linker(&mut linker, |x| x)?;
    wasmtime_wasi::preview2::bindings::cli::stdin::add_to_linker(&mut linker, |x| x)?;
    wasmtime_wasi::preview2::bindings::cli::stdout::add_to_linker(&mut linker, |x| x)?;
    wasmtime_wasi::preview2::bindings::cli::terminal_input::add_to_linker(&mut linker, |x| x)?;
    wasmtime_wasi::preview2::bindings::cli::terminal_output::add_to_linker(&mut linker, |x| x)?;
    wasmtime_wasi::preview2::bindings::cli::terminal_stderr::add_to_linker(&mut linker, |x| x)?;
    wasmtime_wasi::preview2::bindings::cli::terminal_stdin::add_to_linker(&mut linker, |x| x)?;
    wasmtime_wasi::preview2::bindings::cli::terminal_stdout::add_to_linker(&mut linker, |x| x)?;
    wasmtime_wasi::preview2::bindings::clocks::monotonic_clock::add_to_linker(&mut linker, |x| x)?;
    wasmtime_wasi::preview2::bindings::clocks::wall_clock::add_to_linker(&mut linker, |x| x)?;
    wasmtime_wasi::preview2::bindings::filesystem::preopens::add_to_linker(&mut linker, |x| x)?;
    wasmtime_wasi::preview2::bindings::filesystem::types::add_to_linker(&mut linker, |x| x)?;
    wasmtime_wasi::preview2::bindings::io::error::add_to_linker(&mut linker, |x| x)?;
    wasmtime_wasi::preview2::bindings::io::poll::add_to_linker(&mut linker, |x| x)?;
    wasmtime_wasi::preview2::bindings::io::streams::add_to_linker(&mut linker, |x| x)?;
    wasmtime_wasi::preview2::bindings::random::random::add_to_linker(&mut linker, |x| x)?;
    wasmtime_wasi::preview2::bindings::random::insecure::add_to_linker(&mut linker, |x| x)?;
    wasmtime_wasi::preview2::bindings::random::insecure_seed::add_to_linker(&mut linker, |x| x)?;
    wasmtime_wasi::preview2::bindings::sockets::instance_network::add_to_linker(
        &mut linker,
        |x| x,
    )?;
    wasmtime_wasi::preview2::bindings::sockets::network::add_to_linker(&mut linker, |x| x)?;
    wasmtime_wasi::preview2::bindings::sockets::tcp::add_to_linker(&mut linker, |x| x)?;
    wasmtime_wasi::preview2::bindings::sockets::tcp_create_socket::add_to_linker(
        &mut linker,
        |x| x,
    )?;

    wasmtime_wasi_http::bindings::wasi::http::outgoing_handler::add_to_linker(&mut linker, |x| x)?;
    wasmtime_wasi_http::bindings::wasi::http::types::add_to_linker(&mut linker, |x| x)?;

    crate::preview2::wasi::blobstore::blobstore::add_to_linker(&mut linker, |x| x)?;
    crate::preview2::wasi::blobstore::container::add_to_linker(&mut linker, |x| x)?;
    crate::preview2::wasi::blobstore::types::add_to_linker(&mut linker, |x| x)?;

    crate::preview2::wasi::keyvalue::atomic::add_to_linker(&mut linker, |x| x)?;
    crate::preview2::wasi::keyvalue::batch::add_to_linker(&mut linker, |x| x)?;
    crate::preview2::wasi::keyvalue::cache::add_to_linker(&mut linker, |x| x)?;
    crate::preview2::wasi::keyvalue::readwrite::add_to_linker(&mut linker, |x| x)?;
    crate::preview2::wasi::keyvalue::types::add_to_linker(&mut linker, |x| x)?;
    crate::preview2::wasi::keyvalue::wasi_cloud_error::add_to_linker(&mut linker, |x| x)?;

    crate::preview2::golem::api::host::add_to_linker(&mut linker, |x| x)?;

    Ok(linker)
}
