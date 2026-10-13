use futures_concurrency::prelude::*;
use golem_rust::agentic::{Config, Secret};
use golem_rust::bindings::golem::tool::host as tool_host;
use golem_rust::schema::IntoTypedSchemaValue;
use golem_rust::{
    ConfigSchema, FromSchema, FromWire, IntoSchema, IntoWire, WireSchema, agent_definition,
    agent_implementation,
};
use validation_probe_tool_guest_client::ValidationProbeClient;

#[derive(Debug, Clone, IntoSchema, FromSchema, IntoWire, FromWire, WireSchema)]
pub struct ValidationEvidence {
    pub label: String,
    pub ordinal: u64,
    pub stdout: Vec<u8>,
}

#[derive(IntoSchema)]
struct PermissionProbeInput {
    path: String,
}

#[derive(ConfigSchema)]
pub struct ToolRuntimeBypassOwnerConfig {
    #[config_schema(secret)]
    pub context_secret: Secret<String>,
}

#[agent_definition]
pub trait ToolRuntimeBypassOwner {
    fn new(name: String, #[agent_config] config: Config<ToolRuntimeBypassOwnerConfig>) -> Self;
    async fn valid_transformation(&self) -> ValidationEvidence;
    /// Announces `ambient-starting`, starts `permission-probe read <path>` through an
    /// asynchronously lowered `invoke-and-await` detached from the task, logs `ambient-returning`
    /// and returns `returned-early` while that call is still being admitted.
    async fn start_permission_probe_and_exit(&self, path: String) -> String;
    /// Calls `permission-probe read /wait-delete/public`, whose middleware performs an Underlying
    /// `delete /public` after a promise checkpoint, and then an ambient `delete /public`.
    /// Returns both outcomes as `underlying=<outcome>;ambient=<outcome>`.
    async fn underlying_then_ambient_delete(&self) -> String;
}

struct ToolRuntimeBypassOwnerImpl;

#[agent_implementation]
impl ToolRuntimeBypassOwner for ToolRuntimeBypassOwnerImpl {
    fn new(_name: String, #[agent_config] _config: Config<ToolRuntimeBypassOwnerConfig>) -> Self {
        Self
    }

    async fn valid_transformation(&self) -> ValidationEvidence {
        let mut invocation = ValidationProbeClient::default()
            .validate("valid-transform".to_string())
            .await
            .expect("start validation probe");
        let result = invocation.result().await.expect("validation succeeds");
        let mut stdout = Vec::new();
        let mut stream = invocation.stdout.take().expect("declared stdout");
        while let Some(chunk) = stream.next().await {
            stdout.extend(chunk.expect("validation stdout succeeds"));
        }
        ValidationEvidence {
            label: result.nested.label,
            ordinal: result.nested.ordinal,
            stdout,
        }
    }

    async fn underlying_then_ambient_delete(&self) -> String {
        let underlying = permission_probe_outcome("read", "/wait-delete/public").await;
        let ambient = permission_probe_outcome("delete", "/public").await;
        format!("underlying={underlying};ambient={ambient}")
    }

    async fn start_permission_probe_and_exit(&self, path: String) -> String {
        let rpc: &'static tool_host::ToolRpc = Box::leak(Box::new(
            tool_host::ToolRpc::create("permission-probe").expect("bind permission-probe"),
        ));
        let input = golem_rust::encode_typed_schema_value(
            &PermissionProbeInput { path }
                .into_typed_schema_value()
                .expect("permission-probe input schema"),
        )
        .expect("encode permission-probe input");
        announce("ambient-starting").await;
        if detached::start(rpc.invoke_and_await(vec!["read".to_string()], input, None, None, None))
            .is_none()
        {
            return "not-pending".to_string();
        }
        log_marker("ambient-returning");
        "returned-early".to_string()
    }
}

/// Invokes `permission-probe <command> <path>` and describes the outcome as the middleware does:
/// the middleware result on success, `denied` when the ambient admission is denied.
async fn permission_probe_outcome(command: &str, path: &str) -> String {
    let rpc = tool_host::ToolRpc::create("permission-probe").expect("bind permission-probe");
    let input = golem_rust::encode_typed_schema_value(
        &PermissionProbeInput {
            path: path.to_string(),
        }
        .into_typed_schema_value()
        .expect("permission-probe input schema"),
    )
    .expect("encode permission-probe input");
    match rpc
        .invoke_and_await(vec![command.to_string()], input, None, None, None)
        .await
    {
        Ok(result) => match result
            .result
            .as_ref()
            .map(|value| golem_rust::decode_typed_schema_value(value))
        {
            Some(Ok(value)) => match value.value() {
                golem_rust::SchemaValue::String(outcome) => outcome.clone(),
                other => format!("unexpected:{other:?}"),
            },
            other => format!("unexpected:{other:?}"),
        },
        Err(tool_host::ToolRpcError::Denied(_)) => "denied".to_string(),
        Err(error) => format!("error:{error:?}"),
    }
}

async fn announce(path: &str) {
    use golem_rust::wasip3::http::{client, types};
    use golem_rust::wasip3::wit_future;

    let port = std::env::var("EFFECT_PORT").expect("EFFECT_PORT is configured");
    let headers = types::Fields::from_list(&[]).expect("valid effect fields");
    let (trailers_tx, trailers_rx) = wit_future::new(|| Ok(None));
    let (request, transmit) = types::Request::new(headers, None, trailers_rx, None);
    request
        .set_method(&types::Method::Post)
        .expect("set method");
    request
        .set_scheme(Some(&types::Scheme::Http))
        .expect("set scheme");
    request
        .set_authority(Some(&format!("127.0.0.1:{port}")))
        .expect("set authority");
    request
        .set_path_with_query(Some(&format!("/{path}")))
        .expect("set path");
    let send = async move { client::send(request).await.expect("send effect") };
    let finish = async move {
        trailers_tx.write(Ok(None)).await.expect("finish trailers");
        transmit.await.expect("transmit effect");
    };
    let (response, ()) = (send, finish).join().await;
    assert_eq!(response.get_status_code(), 204);
}

/// Starts the first canonical ABI step of an asynchronous import and leaves the call running
/// without any task-local bookkeeping.
///
/// wit-bindgen registers a pending subtask with the current task through the `wasip3_task`
/// interop ABI, and keeps the task from exiting while a registration is outstanding. This polls
/// `future` once under a stand-in `wasip3_task` that only records the subtask handle, then leaks
/// the future and its lowered arguments. The calling task can then return and exit while the
/// subtask is unresolved, which the Component Model allows. Returns the pending subtask handle,
/// or `None` when the call completed or did not start during the first poll.
mod detached {
    use core::ffi::c_void;
    use core::future::Future;
    use core::task::{Context, Waker};

    #[repr(C)]
    struct Wasip3Task {
        version: u32,
        ptr: *mut c_void,
        waitable_register: unsafe extern "C" fn(
            *mut c_void,
            u32,
            unsafe extern "C" fn(*mut c_void, u32),
            *mut c_void,
        ) -> *mut c_void,
        waitable_unregister: unsafe extern "C" fn(*mut c_void, u32) -> *mut c_void,
    }

    unsafe extern "C" {
        fn wasip3_task_set(task: *mut Wasip3Task) -> *mut Wasip3Task;
    }

    unsafe extern "C" fn record_registration(
        ptr: *mut c_void,
        waitable: u32,
        _callback: unsafe extern "C" fn(*mut c_void, u32),
        _callback_ptr: *mut c_void,
    ) -> *mut c_void {
        unsafe { *ptr.cast::<Option<u32>>() = Some(waitable) };
        core::ptr::null_mut()
    }

    unsafe extern "C" fn ignore_unregistration(_ptr: *mut c_void, _waitable: u32) -> *mut c_void {
        core::ptr::null_mut()
    }

    pub fn start<F: Future>(future: F) -> Option<u32> {
        let mut future = Box::pin(future);
        let mut registered: Option<u32> = None;
        let mut task = Wasip3Task {
            version: 1,
            ptr: (&mut registered as *mut Option<u32>).cast(),
            waitable_register: record_registration,
            waitable_unregister: ignore_unregistration,
        };
        let previous = unsafe { wasip3_task_set(&mut task) };
        let pending = future
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
            .is_pending();
        unsafe { wasip3_task_set(previous) };
        core::mem::forget(future);
        if pending { registered } else { None }
    }
}

/// Emits a log line through `wasi:logging`, which does not pass an authority boundary.
fn log_marker(message: &str) {
    golem_rust::bindings::wasi::logging::logging::log(
        golem_rust::bindings::wasi::logging::logging::Level::Info,
        "permission-probe",
        message,
    );
}
