use std::future::Future;
use std::pin::Pin;

pub use golem_common::base_model::agent::Principal;
pub use golem_rust_macro::{
    ToolError, arg, command, constraint, native_tool_definition as tool_definition,
    native_tool_implementation as tool_implementation, result,
};
pub use golem_schema::schema::tool::Tool;
pub use golem_schema::schema::{
    FromSchema, IntoSchema, IntoTypedSchemaValue, SchemaGraph, SchemaType, SchemaValue,
    TypedSchemaValue,
};

pub use golem_common::base_model::tool::TOOL_METADATA_WIT_VERSION as TOOL_METADATA_VERSION;

/// Complete identity shared by registry provisioning and executor installation.
#[derive(Clone, Debug, PartialEq)]
pub struct NativeToolDefinition {
    pub id: String,
    pub implementation_version: String,
    pub tool: Tool,
    pub metadata_version: String,
    pub metadata_digest: [u8; 32],
}

impl NativeToolDefinition {
    pub fn new(
        id: impl Into<String>,
        implementation_version: impl Into<String>,
        tool: Tool,
    ) -> Result<Self, String> {
        let id = id.into();
        let implementation_version = implementation_version.into();
        if id.trim().is_empty() {
            return Err("native tool id cannot be empty".to_string());
        }
        if implementation_version.trim().is_empty() {
            return Err("native tool implementation version cannot be empty".to_string());
        }
        let metadata_version = TOOL_METADATA_VERSION.to_string();
        let metadata_digest =
            *golem_common::model::tool_release::tool_metadata_digest(&metadata_version, &tool)
                .map_err(|error| error.to_string())?
                .as_blake3_hash()
                .as_bytes();
        Ok(Self {
            id,
            implementation_version,
            tool,
            metadata_version,
            metadata_digest,
        })
    }

    pub fn validate(&self) -> Result<(), String> {
        let expected = Self::new(
            self.id.clone(),
            self.implementation_version.clone(),
            self.tool.clone(),
        )?;
        if self.metadata_version != expected.metadata_version
            || self.metadata_digest != expected.metadata_digest
        {
            return Err(format!(
                "native tool '{}@{}' metadata identity does not match its definition",
                self.id, self.implementation_version
            ));
        }
        Ok(())
    }
}

pub mod agentic {
    pub use golem_tool_metadata::native_tool_value_schema as tool_value_schema;
    pub use golem_tool_metadata::*;
}

#[doc(hidden)]
pub mod schema {
    pub use golem_schema::schema::*;

    pub mod tool {
        pub use golem_schema::schema::tool::*;

        pub mod wit {
            pub mod wire {
                pub use golem_schema::schema::tool::*;
            }
        }
    }
}

/// Each operation reborrows the handler's context for host-call admission and durability.
pub trait NativeToolStdinHandle<Ctx>: Send {
    fn read<'a>(&'a mut self, context: &'a mut Ctx) -> NativeToolReadFuture<'a>;
}

pub type NativeToolReadFuture<'a> =
    Pin<Box<dyn Future<Output = Option<Result<Vec<u8>, String>>> + Send + 'a>>;

/// Each operation reborrows the handler's context for host-call admission and durability.
pub trait NativeToolOutputHandle<Ctx>: Send {
    fn write<'a>(
        &'a mut self,
        context: &'a mut Ctx,
        bytes: Vec<u8>,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>>;
    fn finish<'a>(
        &'a mut self,
        context: &'a mut Ctx,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>>;
}

pub type NativeToolStdin<Ctx> = Box<dyn NativeToolStdinHandle<Ctx>>;
pub type NativeToolOutput<Ctx> = Box<dyn NativeToolOutputHandle<Ctx>>;

/// Observation-only view of cancellation requested by the caller of a native tool.
///
/// This handle cannot cancel the invocation. Cancellation is cooperative, and a task awaiting
/// [`Self::cancelled`] is not guaranteed to run cleanup before the executor stops the tool body.
#[derive(Clone)]
pub struct NativeToolCancellation {
    observer: Option<std::sync::Arc<dyn NativeToolCancellationObserver>>,
}

impl NativeToolCancellation {
    /// Returns a handle for an invocation without a live cancellation source, such as completed
    /// replay.
    pub fn unavailable() -> Self {
        Self { observer: None }
    }

    #[doc(hidden)]
    pub fn from_observer(observer: impl NativeToolCancellationObserver) -> Self {
        Self {
            observer: Some(std::sync::Arc::new(observer)),
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.observer
            .as_ref()
            .is_some_and(|observer| observer.is_cancelled())
    }

    pub async fn cancelled(&self) {
        match &self.observer {
            Some(observer) => observer.cancelled().await,
            None => std::future::pending().await,
        }
    }
}

#[doc(hidden)]
pub trait NativeToolCancellationObserver: Send + Sync + 'static {
    fn is_cancelled(&self) -> bool;
    fn cancelled(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>>;
}

pub struct NativeToolInvocation<Ctx> {
    pub command_path: Vec<String>,
    pub input: TypedSchemaValue,
    pub principal: Principal,
    pub cancellation: NativeToolCancellation,
    pub stdin: Option<NativeToolStdin<Ctx>>,
    pub stdout: Option<NativeToolOutput<Ctx>>,
    pub stderr: Option<NativeToolOutput<Ctx>>,
}

#[derive(Debug, PartialEq)]
pub struct NativeToolStructuredResult {
    pub result: Option<TypedSchemaValue>,
}

#[derive(Debug, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum NativeToolRpcError {
    InvalidCommandPath(Vec<String>),
    InvalidInput(String),
    InvalidResult(String),
    Custom {
        name: String,
        payload: TypedSchemaValue,
    },
}

pub type NativeToolRpcResult = Result<NativeToolStructuredResult, NativeToolRpcError>;
/// Infrastructure failures from native host functions. A `Result` nested inside this wrapper is
/// still interpreted as a declared tool error.
pub type HostError = anyhow::Error;
pub type HostResult<T> = Result<T, HostError>;
pub type NativeToolFuture<'a, E> =
    Pin<Box<dyn Future<Output = Result<NativeToolRpcResult, E>> + Send + 'a>>;

/// Generic SDK-side contract. An executor adapter can implement this directly for
/// its concrete worker context without type erasure or downcasting.
pub trait NativeToolInvoker<Ctx, E = std::convert::Infallible>: Send + Sync + 'static {
    fn metadata(&self) -> Tool;

    fn definition(
        &self,
        id: impl Into<String>,
        implementation_version: impl Into<String>,
    ) -> Result<NativeToolDefinition, String>
    where
        Self: Sized,
    {
        NativeToolDefinition::new(id, implementation_version, self.metadata())
    }

    fn invoke<'a>(
        &'a self,
        context: &'a mut Ctx,
        invocation: NativeToolInvocation<Ctx>,
    ) -> NativeToolFuture<'a, E>;
}

/// Deterministic ambient native tool used by CLI conformance tests.
///
/// The production catalogs only install it when [`TEST_FIXTURE_ENV`] is set. Keeping the
/// definition and implementation here gives the registry and executor one metadata identity.
pub mod conformance_fixture {
    use super::*;
    use std::marker::PhantomData;

    pub const TEST_FIXTURE_ENV: &str = "GOLEM_TEST_NATIVE_CONFORMANCE_TOOL";
    pub const TOOL_NAME: &str = "native-conformance";
    pub const HOST_TOOL_ID: &str = "native-conformance-fixture";
    pub const IMPLEMENTATION_VERSION: &str = "1.0.0";
    pub const REFRESHED_IMPLEMENTATION_VERSION: &str = "2.0.0";

    #[derive(Debug, Clone, IntoSchema)]
    pub struct Evidence {
        pub value: String,
        pub count: u64,
        pub agent_authorized: bool,
    }

    #[derive(Debug, ToolError)]
    pub enum ConformanceError {
        #[tool_error(kind = "usage-error", exit_code = 2)]
        Rejected { reason: String },
    }

    #[tool_definition(version = "1.0.0")]
    trait NativeConformance<Ctx: Send> {
        fn structured(
            &self,
            context: &mut Ctx,
            value: String,
            count: u64,
            principal: golem_native_tool::Principal,
        ) -> Evidence;

        fn supported_error(
            &self,
            context: &mut Ctx,
            reason: String,
        ) -> Result<Evidence, ConformanceError>;

        async fn finite_stream(
            &self,
            context: &mut Ctx,
            value: String,
            stdout: NativeToolOutput<Ctx>,
        ) -> Evidence;

        fn middleware(&self, context: &mut Ctx, value: String) -> String;
    }

    struct NativeConformanceImpl<Ctx>(PhantomData<fn(Ctx)>);

    #[tool_implementation]
    impl<Ctx: Send + 'static> NativeConformance<Ctx> for NativeConformanceImpl<Ctx> {
        fn structured(
            &self,
            _context: &mut Ctx,
            value: String,
            count: u64,
            _principal: Principal,
        ) -> Evidence {
            Evidence {
                value,
                count,
                agent_authorized: true,
            }
        }

        fn supported_error(
            &self,
            _context: &mut Ctx,
            reason: String,
        ) -> Result<Evidence, ConformanceError> {
            Err(ConformanceError::Rejected { reason })
        }

        async fn finite_stream(
            &self,
            context: &mut Ctx,
            value: String,
            mut stdout: NativeToolOutput<Ctx>,
        ) -> Evidence {
            stdout
                .write(context, format!("first:{value}|").into_bytes())
                .await
                .unwrap();
            stdout.write(context, b"second".to_vec()).await.unwrap();
            stdout.finish(context).await.unwrap();
            Evidence {
                value,
                count: 2,
                agent_authorized: true,
            }
        }

        fn middleware(&self, _context: &mut Ctx, value: String) -> String {
            format!("leaf({value})")
        }
    }

    #[tool_definition(version = "2.0.0")]
    trait NativeConformanceRefreshed<Ctx: Send> {
        fn refreshed(&self, context: &mut Ctx) -> u64;
    }

    struct NativeConformanceRefreshedImpl<Ctx>(PhantomData<fn(Ctx)>);

    #[tool_implementation]
    impl<Ctx: Send + 'static> NativeConformanceRefreshed<Ctx> for NativeConformanceRefreshedImpl<Ctx> {
        fn refreshed(&self, _context: &mut Ctx) -> u64 {
            2
        }
    }

    pub struct ConformanceNativeTool;

    pub fn enabled() -> bool {
        std::env::var_os(TEST_FIXTURE_ENV).is_some()
    }

    fn refreshed() -> bool {
        std::env::var(TEST_FIXTURE_ENV).is_ok_and(|value| value == "2")
    }

    pub fn definition() -> NativeToolDefinition {
        if refreshed() {
            let mut tool = NativeConformanceRefreshedImpl::<()>(PhantomData)
                .native_tool_invoker()
                .metadata();
            tool.commands.nodes[0].name = TOOL_NAME.to_string();
            NativeToolDefinition::new(HOST_TOOL_ID, REFRESHED_IMPLEMENTATION_VERSION, tool)
                .expect("refreshed native conformance fixture definition is valid")
        } else {
            NativeConformanceImpl::<()>(PhantomData)
                .native_tool_invoker()
                .definition(HOST_TOOL_ID, IMPLEMENTATION_VERSION)
                .expect("native conformance fixture definition is valid")
        }
    }

    impl<Ctx> NativeToolInvoker<Ctx, anyhow::Error> for ConformanceNativeTool
    where
        Ctx: Send + 'static,
    {
        fn metadata(&self) -> Tool {
            definition().tool
        }

        fn invoke<'a>(
            &'a self,
            context: &'a mut Ctx,
            invocation: NativeToolInvocation<Ctx>,
        ) -> NativeToolFuture<'a, anyhow::Error> {
            Box::pin(async move {
                if refreshed() {
                    NativeConformanceRefreshedImpl::<Ctx>(PhantomData)
                        .native_tool_invoker()
                        .invoke(context, invocation)
                        .await
                } else {
                    NativeConformanceImpl::<Ctx>(PhantomData)
                        .native_tool_invoker()
                        .invoke(context, invocation)
                        .await
                }
            })
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::cell::Cell;
        use test_r::test;

        struct Context {
            bytes: Vec<u8>,
            finished: Cell<bool>,
        }

        struct Output;

        impl NativeToolOutputHandle<Context> for Output {
            fn write<'a>(
                &'a mut self,
                context: &'a mut Context,
                bytes: Vec<u8>,
            ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
                Box::pin(async move {
                    context.bytes.extend(bytes);
                    Ok(())
                })
            }

            fn finish<'a>(
                &'a mut self,
                context: &'a mut Context,
            ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
                Box::pin(async move {
                    context.finished.set(true);
                    Ok(())
                })
            }
        }

        #[test]
        async fn conformance_stream_uses_non_sync_context_without_changing_metadata() {
            let invoker = NativeConformanceImpl::<Context>(PhantomData).native_tool_invoker();
            let tool = invoker.metadata();
            assert_eq!(
                tool,
                NativeConformanceImpl::<()>(PhantomData)
                    .native_tool_invoker()
                    .metadata()
            );
            let index = tool
                .command_index_by_path(&["finite-stream".into()])
                .unwrap();
            let model = tool.canonical_input_model(index).unwrap();
            let mut context = Context {
                bytes: b"prefix:".to_vec(),
                finished: Cell::new(false),
            };
            let result = invoker
                .invoke(
                    &mut context,
                    NativeToolInvocation {
                        command_path: vec!["finite-stream".into()],
                        input: TypedSchemaValue::new(
                            model.record_schema,
                            SchemaValue::Record {
                                fields: vec![SchemaValue::String("value".into())],
                            },
                        ),
                        principal: Principal::anonymous(),
                        cancellation: NativeToolCancellation::unavailable(),
                        stdin: None,
                        stdout: Some(Box::new(Output)),
                        stderr: None,
                    },
                )
                .await
                .unwrap()
                .unwrap();
            assert_eq!(context.bytes, b"prefix:first:value|second");
            assert!(context.finished.get());
            assert_eq!(
                result.result.unwrap(),
                Evidence {
                    value: "value".into(),
                    count: 2,
                    agent_authorized: true,
                }
                .into_typed_schema_value()
                .unwrap()
            );
        }
    }
}

#[doc(hidden)]
pub fn encode_result<T: IntoSchema + ?Sized>(
    value: &T,
) -> Result<TypedSchemaValue, NativeToolRpcError> {
    value
        .into_typed_schema_value()
        .map_err(|error| NativeToolRpcError::InvalidResult(error.to_string()))
}

#[cfg(test)]
test_r::enable!();
