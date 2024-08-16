use std::time::Duration;

use async_trait::async_trait;
use futures_util::{future, pin_mut, SinkExt, StreamExt};
use golem_cli::clients::worker::WorkerClient;
use golem_cli::command::worker::WorkerConnectOptions;
use golem_cli::connect_output::ConnectOutput;
use golem_client::model::ScanCursor;
use golem_cloud_client::model::{
    AnalysedResourceMode, AnalysedType, FilterComparator, InvokeParameters, InvokeResult,
    NameOptionTypePair, NameTypePair, StringFilterComparator, TypeAnnotatedValue, TypeBool,
    TypeChr, TypeEnum, TypeF32, TypeF64, TypeFlags, TypeHandle, TypeList, TypeOption, TypeRecord,
    TypeResult, TypeS16, TypeS32, TypeS64, TypeS8, TypeStr, TypeTuple, TypeU16, TypeU32, TypeU64,
    TypeU8, TypeVariant, WorkerAndFilter, WorkerCreatedAtFilter, WorkerCreationRequest,
    WorkerEnvFilter, WorkerFilter, WorkerNameFilter, WorkerNotFilter, WorkerOrFilter, WorkerStatus,
    WorkerStatusFilter, WorkerVersionFilter, WorkersMetadataRequest,
};
use golem_cloud_client::Context;
use golem_cloud_client::Error;
use native_tls::TlsConnector;
use tokio::{task, time};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::Message;
use tokio_tungstenite::{connect_async_tls_with_config, Connector};
use tracing::{debug, error, info, trace};

use crate::cloud::clients::errors::CloudGolemError;
use crate::cloud::model::{to_oss_type, ToCli, ToCloud, ToOss};

use golem_cli::model::{
    Format, GolemError, IdempotencyKey, WorkerMetadata, WorkerName, WorkerUpdateMode,
};
use golem_cloud_client::api::WorkerError;
use golem_common::model::WorkerEvent;
use golem_common::uri::oss::urn::{ComponentUrn, WorkerUrn};

#[derive(Clone)]
pub struct WorkerClientLive<C: golem_cloud_client::api::WorkerClient + Sync + Send> {
    pub client: C,
    pub context: Context,
    pub allow_insecure: bool,
}

fn to_cloud_invoke_parameters(ps: golem_client::model::InvokeParameters) -> InvokeParameters {
    fn to_cloud_name_option_type_pair(
        p: golem_client::model::NameOptionTypePair,
    ) -> NameOptionTypePair {
        NameOptionTypePair {
            name: p.name,
            typ: p.typ.map(to_cloud_analysed_type),
        }
    }

    fn to_cloud_variant(v: golem_client::model::TypeVariant) -> TypeVariant {
        TypeVariant {
            cases: v
                .cases
                .into_iter()
                .map(to_cloud_name_option_type_pair)
                .collect(),
        }
    }

    fn to_cloud_result(r: golem_client::model::TypeResult) -> TypeResult {
        TypeResult {
            ok: r.ok.map(to_cloud_analysed_type),
            err: r.err.map(to_cloud_analysed_type),
        }
    }

    fn to_cloud_option(o: golem_client::model::TypeOption) -> TypeOption {
        TypeOption {
            inner: to_cloud_analysed_type(o.inner),
        }
    }

    fn to_cloud_enum(e: golem_client::model::TypeEnum) -> TypeEnum {
        TypeEnum { cases: e.cases }
    }

    fn to_cloud_flags(f: golem_client::model::TypeFlags) -> TypeFlags {
        TypeFlags { names: f.names }
    }

    fn to_cloud_name_type_pair(p: golem_client::model::NameTypePair) -> NameTypePair {
        NameTypePair {
            name: p.name,
            typ: to_cloud_analysed_type(p.typ),
        }
    }

    fn to_cloud_record(r: golem_client::model::TypeRecord) -> TypeRecord {
        TypeRecord {
            fields: r.fields.into_iter().map(to_cloud_name_type_pair).collect(),
        }
    }

    fn to_cloud_tuple(r: golem_client::model::TypeTuple) -> TypeTuple {
        TypeTuple {
            items: r.items.into_iter().map(to_cloud_analysed_type).collect(),
        }
    }

    fn to_cloud_list(l: golem_client::model::TypeList) -> TypeList {
        TypeList {
            inner: to_cloud_analysed_type(l.inner),
        }
    }

    fn to_cloud_mode(m: golem_client::model::AnalysedResourceMode) -> AnalysedResourceMode {
        match m {
            golem_client::model::AnalysedResourceMode::Owned => AnalysedResourceMode::Owned,
            golem_client::model::AnalysedResourceMode::Borrowed => AnalysedResourceMode::Borrowed,
        }
    }

    fn to_cloud_type_handle(h: golem_client::model::TypeHandle) -> TypeHandle {
        TypeHandle {
            resource_id: h.resource_id,
            mode: to_cloud_mode(h.mode),
        }
    }

    fn to_cloud_analysed_type(t: golem_client::model::AnalysedType) -> AnalysedType {
        match t {
            golem_client::model::AnalysedType::Variant(v) => {
                AnalysedType::Variant(to_cloud_variant(v))
            }
            golem_client::model::AnalysedType::Result(r) => {
                AnalysedType::Result(Box::new(to_cloud_result(*r)))
            }
            golem_client::model::AnalysedType::Option(o) => {
                AnalysedType::Option(Box::new(to_cloud_option(*o)))
            }
            golem_client::model::AnalysedType::Enum(e) => AnalysedType::Enum(to_cloud_enum(e)),
            golem_client::model::AnalysedType::Flags(f) => AnalysedType::Flags(to_cloud_flags(f)),
            golem_client::model::AnalysedType::Record(r) => {
                AnalysedType::Record(to_cloud_record(r))
            }
            golem_client::model::AnalysedType::Tuple(t) => AnalysedType::Tuple(to_cloud_tuple(t)),
            golem_client::model::AnalysedType::List(l) => {
                AnalysedType::List(Box::new(to_cloud_list(*l)))
            }
            golem_client::model::AnalysedType::Str(_) => AnalysedType::Str(TypeStr {}),
            golem_client::model::AnalysedType::Chr(_) => AnalysedType::Chr(TypeChr {}),
            golem_client::model::AnalysedType::F64(_) => AnalysedType::F64(TypeF64 {}),
            golem_client::model::AnalysedType::F32(_) => AnalysedType::F32(TypeF32 {}),
            golem_client::model::AnalysedType::U64(_) => AnalysedType::U64(TypeU64 {}),
            golem_client::model::AnalysedType::S64(_) => AnalysedType::S64(TypeS64 {}),
            golem_client::model::AnalysedType::U32(_) => AnalysedType::U32(TypeU32 {}),
            golem_client::model::AnalysedType::S32(_) => AnalysedType::S32(TypeS32 {}),
            golem_client::model::AnalysedType::U16(_) => AnalysedType::U16(TypeU16 {}),
            golem_client::model::AnalysedType::S16(_) => AnalysedType::S16(TypeS16 {}),
            golem_client::model::AnalysedType::U8(_) => AnalysedType::U8(TypeU8 {}),
            golem_client::model::AnalysedType::S8(_) => AnalysedType::S8(TypeS8 {}),
            golem_client::model::AnalysedType::Bool(_) => AnalysedType::Bool(TypeBool {}),
            golem_client::model::AnalysedType::Handle(h) => {
                AnalysedType::Handle(to_cloud_type_handle(h))
            }
        }
    }

    fn to_cloud_type_annotated_value(
        v: golem_client::model::TypeAnnotatedValue,
    ) -> TypeAnnotatedValue {
        TypeAnnotatedValue {
            typ: to_cloud_analysed_type(v.typ),
            value: v.value,
        }
    }

    InvokeParameters {
        params: ps
            .params
            .into_iter()
            .map(to_cloud_type_annotated_value)
            .collect(),
    }
}

fn to_oss_invoke_result(r: InvokeResult) -> golem_client::model::InvokeResult {
    golem_client::model::InvokeResult {
        result: golem_client::model::TypeAnnotatedValue {
            typ: to_oss_type(r.result.typ),
            value: r.result.value,
        },
    }
}

fn to_cloud_worker_filter(f: golem_client::model::WorkerFilter) -> WorkerFilter {
    fn to_cloud_string_filter_comparator(
        c: &golem_client::model::StringFilterComparator,
    ) -> StringFilterComparator {
        match c {
            golem_client::model::StringFilterComparator::Equal => StringFilterComparator::Equal,
            golem_client::model::StringFilterComparator::NotEqual => {
                StringFilterComparator::NotEqual
            }
            golem_client::model::StringFilterComparator::Like => StringFilterComparator::Like,
            golem_client::model::StringFilterComparator::NotLike => StringFilterComparator::NotLike,
        }
    }

    fn to_cloud_filter_name(f: golem_client::model::WorkerNameFilter) -> WorkerNameFilter {
        WorkerNameFilter {
            comparator: to_cloud_string_filter_comparator(&f.comparator),
            value: f.value,
        }
    }

    fn to_cloud_filter_comparator(f: &golem_client::model::FilterComparator) -> FilterComparator {
        match f {
            golem_client::model::FilterComparator::Equal => FilterComparator::Equal,
            golem_client::model::FilterComparator::NotEqual => FilterComparator::NotEqual,
            golem_client::model::FilterComparator::GreaterEqual => FilterComparator::GreaterEqual,
            golem_client::model::FilterComparator::Greater => FilterComparator::Greater,
            golem_client::model::FilterComparator::LessEqual => FilterComparator::LessEqual,
            golem_client::model::FilterComparator::Less => FilterComparator::Less,
        }
    }

    fn to_cloud_worker_status(s: &golem_client::model::WorkerStatus) -> WorkerStatus {
        match s {
            golem_client::model::WorkerStatus::Running => WorkerStatus::Running,
            golem_client::model::WorkerStatus::Idle => WorkerStatus::Idle,
            golem_client::model::WorkerStatus::Suspended => WorkerStatus::Suspended,
            golem_client::model::WorkerStatus::Interrupted => WorkerStatus::Interrupted,
            golem_client::model::WorkerStatus::Retrying => WorkerStatus::Retrying,
            golem_client::model::WorkerStatus::Failed => WorkerStatus::Failed,
            golem_client::model::WorkerStatus::Exited => WorkerStatus::Exited,
        }
    }

    fn to_cloud_filter_status(f: golem_client::model::WorkerStatusFilter) -> WorkerStatusFilter {
        WorkerStatusFilter {
            comparator: to_cloud_filter_comparator(&f.comparator),
            value: to_cloud_worker_status(&f.value),
        }
    }
    fn to_cloud_filter_version(f: golem_client::model::WorkerVersionFilter) -> WorkerVersionFilter {
        WorkerVersionFilter {
            comparator: to_cloud_filter_comparator(&f.comparator),
            value: f.value,
        }
    }
    fn to_cloud_filter_created_at(
        f: golem_client::model::WorkerCreatedAtFilter,
    ) -> WorkerCreatedAtFilter {
        WorkerCreatedAtFilter {
            comparator: to_cloud_filter_comparator(&f.comparator),
            value: f.value,
        }
    }
    fn to_cloud_filter_env(f: golem_client::model::WorkerEnvFilter) -> WorkerEnvFilter {
        let golem_client::model::WorkerEnvFilter {
            name,
            comparator,
            value,
        } = f;

        WorkerEnvFilter {
            name,
            comparator: to_cloud_string_filter_comparator(&comparator),
            value,
        }
    }
    fn to_cloud_filter_and(f: golem_client::model::WorkerAndFilter) -> WorkerAndFilter {
        WorkerAndFilter {
            filters: f.filters.into_iter().map(to_cloud_worker_filter).collect(),
        }
    }
    fn to_cloud_filter_or(f: golem_client::model::WorkerOrFilter) -> WorkerOrFilter {
        WorkerOrFilter {
            filters: f.filters.into_iter().map(to_cloud_worker_filter).collect(),
        }
    }
    fn to_cloud_filter_not(f: golem_client::model::WorkerNotFilter) -> WorkerNotFilter {
        WorkerNotFilter {
            filter: to_cloud_worker_filter(f.filter),
        }
    }

    match f {
        golem_client::model::WorkerFilter::Name(f) => WorkerFilter::Name(to_cloud_filter_name(f)),
        golem_client::model::WorkerFilter::Status(f) => {
            WorkerFilter::Status(to_cloud_filter_status(f))
        }
        golem_client::model::WorkerFilter::Version(f) => {
            WorkerFilter::Version(to_cloud_filter_version(f))
        }
        golem_client::model::WorkerFilter::CreatedAt(f) => {
            WorkerFilter::CreatedAt(to_cloud_filter_created_at(f))
        }
        golem_client::model::WorkerFilter::Env(f) => WorkerFilter::Env(to_cloud_filter_env(f)),
        golem_client::model::WorkerFilter::And(f) => WorkerFilter::And(to_cloud_filter_and(f)),
        golem_client::model::WorkerFilter::Or(f) => WorkerFilter::Or(to_cloud_filter_or(f)),
        golem_client::model::WorkerFilter::Not(f) => {
            WorkerFilter::Not(Box::new(to_cloud_filter_not(*f)))
        }
    }
}

#[async_trait]
impl<C: golem_cloud_client::api::WorkerClient + Sync + Send> WorkerClient for WorkerClientLive<C> {
    async fn new_worker(
        &self,
        name: WorkerName,
        component_urn: ComponentUrn,
        args: Vec<String>,
        env: Vec<(String, String)>,
    ) -> Result<golem_client::model::WorkerId, GolemError> {
        info!("Creating worker {name} of {}", component_urn.id.0);

        Ok(self
            .client
            .launch_new_worker(
                &component_urn.id.0,
                &WorkerCreationRequest {
                    name: name.0,
                    args,
                    env: env.into_iter().collect(),
                },
            )
            .await
            .map_err(CloudGolemError::from)?
            .worker_id
            .to_oss())
    }

    async fn invoke_and_await(
        &self,
        worker_urn: WorkerUrn,
        function: String,
        parameters: golem_client::model::InvokeParameters,
        idempotency_key: Option<IdempotencyKey>,
    ) -> Result<golem_client::model::InvokeResult, GolemError> {
        info!("Invoke and await for function {function} in {worker_urn}");

        Ok(to_oss_invoke_result(
            self.client
                .invoke_and_await_function(
                    &worker_urn.id.component_id.0,
                    &worker_urn.id.worker_name,
                    idempotency_key.as_ref().map(|k| k.0.as_str()),
                    &function,
                    &to_cloud_invoke_parameters(parameters),
                )
                .await
                .map_err(CloudGolemError::from)?,
        ))
    }

    async fn invoke(
        &self,
        worker_urn: WorkerUrn,
        function: String,
        parameters: golem_client::model::InvokeParameters,
        idempotency_key: Option<IdempotencyKey>,
    ) -> Result<(), GolemError> {
        info!("Invoke function {function} in {worker_urn}");

        let _ = self
            .client
            .invoke_function(
                &worker_urn.id.component_id.0,
                &worker_urn.id.worker_name,
                idempotency_key.as_ref().map(|k| k.0.as_str()),
                &function,
                &to_cloud_invoke_parameters(parameters),
            )
            .await
            .map_err(CloudGolemError::from)?;
        Ok(())
    }

    async fn interrupt(&self, worker_urn: WorkerUrn) -> Result<(), GolemError> {
        info!("Interrupting {worker_urn}");

        let _ = self
            .client
            .interrupt_worker(
                &worker_urn.id.component_id.0,
                &worker_urn.id.worker_name,
                Some(false),
            )
            .await
            .map_err(CloudGolemError::from)?;
        Ok(())
    }

    async fn resume(&self, worker_urn: WorkerUrn) -> Result<(), GolemError> {
        info!("Resuming {worker_urn}");

        let _ = self
            .client
            .resume_worker(&worker_urn.id.component_id.0, &worker_urn.id.worker_name)
            .await
            .map_err(CloudGolemError::from)?;
        Ok(())
    }

    async fn simulated_crash(&self, worker_urn: WorkerUrn) -> Result<(), GolemError> {
        info!("Simulating crash of {worker_urn}");

        let _ = self
            .client
            .interrupt_worker(
                &worker_urn.id.component_id.0,
                &worker_urn.id.worker_name,
                Some(true),
            )
            .await
            .map_err(CloudGolemError::from)?;
        Ok(())
    }

    async fn delete(&self, worker_urn: WorkerUrn) -> Result<(), GolemError> {
        info!("Deleting worker {worker_urn}");

        let _ = self
            .client
            .delete_worker(&worker_urn.id.component_id.0, &worker_urn.id.worker_name)
            .await
            .map_err(CloudGolemError::from)?;
        Ok(())
    }

    async fn get_metadata(&self, worker_urn: WorkerUrn) -> Result<WorkerMetadata, GolemError> {
        info!("Getting worker {worker_urn} metadata");

        Ok(self
            .client
            .get_worker_metadata(&worker_urn.id.component_id.0, &worker_urn.id.worker_name)
            .await
            .map_err(CloudGolemError::from)?
            .to_cli())
    }

    async fn find_metadata(
        &self,
        component_urn: ComponentUrn,
        filter: Option<golem_client::model::WorkerFilter>,
        cursor: Option<ScanCursor>,
        count: Option<u64>,
        precise: Option<bool>,
    ) -> Result<golem_cli::model::WorkersMetadataResponse, GolemError> {
        info!(
            "Getting workers metadata for component: {component_urn}, filter: {}",
            filter.is_some()
        );

        Ok(self
            .client
            .find_workers_metadata(
                &component_urn.id.0,
                &WorkersMetadataRequest {
                    filter: filter.map(to_cloud_worker_filter),
                    cursor: cursor.map(|c| c.to_cloud()),
                    count,
                    precise,
                },
            )
            .await
            .map_err(CloudGolemError::from)?
            .to_cli())
    }

    async fn list_metadata(
        &self,
        component_urn: ComponentUrn,
        filter: Option<Vec<String>>,
        cursor: Option<ScanCursor>,
        count: Option<u64>,
        precise: Option<bool>,
    ) -> Result<golem_cli::model::WorkersMetadataResponse, GolemError> {
        info!(
            "Getting workers metadata for component: {component_urn}, filter: {}",
            filter
                .clone()
                .map(|fs| fs.join(" AND "))
                .unwrap_or("N/A".to_string())
        );

        let filter: Option<&[String]> = filter.as_deref();

        let cursor = cursor.map(|cursor| format!("{}/{}", cursor.layer, cursor.cursor));

        Ok(self
            .client
            .get_workers_metadata(
                &component_urn.id.0,
                filter,
                cursor.as_deref(),
                count,
                precise,
            )
            .await
            .map_err(CloudGolemError::from)?
            .to_cli())
    }

    async fn connect(
        &self,
        worker_urn: WorkerUrn,
        connect_options: WorkerConnectOptions,
        format: Format,
    ) -> Result<(), GolemError> {
        let mut url = self.context.base_url.clone();

        let ws_schema = if url.scheme() == "http" { "ws" } else { "wss" };

        url.set_scheme(ws_schema)
            .map_err(|_| GolemError("Can't set schema.".to_string()))?;

        url.path_segments_mut()
            .map_err(|_| GolemError("Can't get path.".to_string()))?
            .push("v1")
            .push("components")
            .push(&worker_urn.id.component_id.0.to_string())
            .push("workers")
            .push(&worker_urn.id.worker_name)
            .push("connect");

        let mut request = url
            .into_client_request()
            .map_err(|e| GolemError(format!("Can't create request: {e}")))?;
        let headers = request.headers_mut();

        if let Some(token) = self.context.bearer_token() {
            headers.insert(
                "Authorization",
                format!("Bearer {}", token).parse().unwrap(),
            );
        }

        let connector = if self.allow_insecure {
            Some(Connector::NativeTls(
                TlsConnector::builder()
                    .danger_accept_invalid_certs(true)
                    .danger_accept_invalid_hostnames(true)
                    .build()
                    .unwrap(),
            ))
        } else {
            None
        };

        info!("Connecting to {worker_urn}");

        let (ws_stream, _) = connect_async_tls_with_config(request, None, false, connector)
            .await
            .map_err(|e| match e {
                tungstenite::error::Error::Http(http_error_response) => {
                    let status = http_error_response.status().as_u16();
                    match http_error_response.body().clone() {
                        Some(body) => get_worker_golem_error(status, body),
                        None => GolemError(format!("Failed Websocket. Http error: {}", status)),
                    }
                }
                _ => GolemError(format!("Failed Websocket. Error: {}", e)),
            })?;

        let (mut write, read) = ws_stream.split();

        let pings = task::spawn(async move {
            let mut interval = time::interval(Duration::from_secs(1)); // TODO configure
            let mut cnt: i32 = 1;

            loop {
                interval.tick().await;

                let ping_result = write
                    .send(Message::Ping(cnt.to_ne_bytes().to_vec()))
                    .await
                    .map_err(|err| GolemError(format!("Worker connection ping failure: {err}")));

                if let Err(err) = ping_result {
                    error!("{}", err);
                    break err;
                }

                cnt += 1;
            }
        });

        let output = ConnectOutput::new(connect_options, format);

        let read_res = read.for_each(move |message_or_error| {
            let output = output.clone();
            async move {
                match message_or_error {
                    Err(error) => {
                        error!("Error reading message: {}", error);
                    }
                    Ok(message) => {
                        let instance_connect_msg = match message {
                            Message::Text(str) => {
                                let parsed: serde_json::Result<WorkerEvent> =
                                    serde_json::from_str(&str);

                                match parsed {
                                    Ok(parsed) => Some(parsed),
                                    Err(err) => {
                                        error!("Failed to parse worker connect message: {err}");
                                        None
                                    }
                                }
                            }
                            Message::Binary(data) => {
                                let parsed: serde_json::Result<WorkerEvent> =
                                    serde_json::from_slice(&data);
                                match parsed {
                                    Ok(parsed) => Some(parsed),
                                    Err(err) => {
                                        error!("Failed to parse worker connect message: {err}");
                                        None
                                    }
                                }
                            }
                            Message::Ping(_) => {
                                trace!("Ignore ping");
                                None
                            }
                            Message::Pong(_) => {
                                trace!("Ignore pong");
                                None
                            }
                            Message::Close(details) => {
                                match details {
                                    Some(closed_frame) => {
                                        info!("Connection Closed: {}", closed_frame);
                                    }
                                    None => {
                                        info!("Connection Closed");
                                    }
                                }
                                None
                            }
                            Message::Frame(f) => {
                                debug!("Ignored unexpected frame {f:?}");
                                None
                            }
                        };

                        match instance_connect_msg {
                            None => {}
                            Some(msg) => match msg {
                                WorkerEvent::StdOut { timestamp, bytes } => {
                                    output
                                        .emit_stdout(
                                            timestamp,
                                            String::from_utf8_lossy(&bytes).to_string(),
                                        )
                                        .await;
                                }
                                WorkerEvent::StdErr { timestamp, bytes } => {
                                    output
                                        .emit_stderr(
                                            timestamp,
                                            String::from_utf8_lossy(&bytes).to_string(),
                                        )
                                        .await;
                                }
                                WorkerEvent::Log {
                                    timestamp,
                                    level,
                                    context,
                                    message,
                                } => {
                                    output.emit_log(timestamp, level, context, message);
                                }
                                WorkerEvent::Close => {}
                            },
                        }
                    }
                }
            }
        });

        pin_mut!(read_res, pings);

        future::select(pings, read_res).await;
        Ok(())
    }

    async fn update(
        &self,
        worker_urn: WorkerUrn,
        mode: WorkerUpdateMode,
        target_version: u64,
    ) -> Result<(), GolemError> {
        info!("Updating worker {worker_urn}");
        let update_mode = match mode {
            WorkerUpdateMode::Automatic => golem_cloud_client::model::WorkerUpdateMode::Automatic,
            WorkerUpdateMode::Manual => golem_cloud_client::model::WorkerUpdateMode::Manual,
        };

        let _ = self
            .client
            .update_worker(
                &worker_urn.id.component_id.0,
                &worker_urn.id.worker_name,
                &golem_cloud_client::model::UpdateWorkerRequest {
                    mode: update_mode,
                    target_version,
                },
            )
            .await
            .map_err(CloudGolemError::from)?;
        Ok(())
    }
}

fn get_worker_golem_error(status: u16, body: Vec<u8>) -> GolemError {
    let error: Result<Error<WorkerError>, serde_json::Error> = match status {
        400 => serde_json::from_slice(&body).map(|body| Error::Item(WorkerError::Error400(body))),
        401 => serde_json::from_slice(&body).map(|body| Error::Item(WorkerError::Error401(body))),
        403 => serde_json::from_slice(&body).map(|body| Error::Item(WorkerError::Error403(body))),
        404 => serde_json::from_slice(&body).map(|body| Error::Item(WorkerError::Error404(body))),
        409 => serde_json::from_slice(&body).map(|body| Error::Item(WorkerError::Error409(body))),
        500 => serde_json::from_slice(&body).map(|body| Error::Item(WorkerError::Error500(body))),
        _ => Ok(Error::unexpected(status, body.into())),
    };
    CloudGolemError::from(error.unwrap_or_else(Error::from)).into()
}
