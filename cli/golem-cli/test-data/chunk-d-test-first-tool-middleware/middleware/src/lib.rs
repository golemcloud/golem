use golem_rust::tool::{
    InputStream, OutputStream, Principal, Tool, ToolInvokeError, ToolMiddleware,
    ToolMiddlewareInvokeFuture, ToolMiddlewareScope, UnderlyingTool,
};
use golem_rust::{
    FromSchema, FromWire, IntoSchema, IntoWire, ToolError, TypedSchemaValue, WireSchema,
    tool_definition, tool_middleware,
};
use std::io::Write;

mod provider;
mod rich_provider;

fn record(marker: &str) {
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("/chunk-d-effects.log")
        .and_then(|mut file| file.write_all(marker.as_bytes()))
        .expect("middleware occurrence has owner filesystem access");
}

#[derive(Debug, Clone, ToolError)]
pub enum ProbeError {
    #[tool_error(kind = "usage-error", exit_code = 11)]
    Rejected { value: String },
    #[tool_error(kind = "runtime-error", exit_code = 12)]
    Transformed { value: String },
}

#[tool_definition(version = "1.0.0")]
pub trait ManifestProbe {
    async fn apply(&self, value: String) -> Result<String, ProbeError>;
}

#[derive(IntoSchema, FromSchema)]
struct LabelParameters {
    label: String,
}

struct LabelLayer {
    parameters: LabelParameters,
}

impl LabelLayer {
    fn new(parameters: LabelParameters) -> Self {
        Self { parameters }
    }
}

#[tool_middleware(
    name = "manifest-label-layer",
    constructor = LabelLayer::new,
    parameters = LabelParameters
)]
impl ManifestProbeMiddleware for LabelLayer {
    async fn apply(
        &self,
        underlying: &ManifestProbeUnderlying,
        value: String,
    ) -> Result<String, ToolInvokeError<ProbeError>> {
        record(&format!("{};", self.parameters.label));
        let result = underlying
            .apply(format!("{}({value})", self.parameters.label))
            .await?;
        Ok(format!("{}[{result}]", self.parameters.label))
    }
}

struct ControlLayer;

impl ControlLayer {
    fn new() -> Self {
        Self
    }
}

#[tool_middleware(name = "control-retry-transform", constructor = ControlLayer::new)]
impl ManifestProbeMiddleware for ControlLayer {
    async fn apply(
        &self,
        underlying: &ManifestProbeUnderlying,
        value: String,
    ) -> Result<String, ToolInvokeError<ProbeError>> {
        let transformed = match underlying.apply(format!("reject({value})")).await {
            Err(ToolInvokeError::Tool(ProbeError::Rejected { value })) => {
                format!("transformed({value})")
            }
            result => return result,
        };
        let success = underlying.apply(format!("retry({value})")).await?;
        Ok(format!("{transformed}|{success}"))
    }
}

fn invoke_universal(
    _tool_name: String,
    _tool_metadata: Tool,
    parameters: TypedSchemaValue,
    command_path: Vec<String>,
    input: TypedSchemaValue,
    stdin: Option<InputStream>,
    stdout: Option<OutputStream>,
    stderr: Option<OutputStream>,
    _principal: Principal,
    underlying: UnderlyingTool,
) -> ToolMiddlewareInvokeFuture {
    Box::pin(async move {
        let parameters = LabelParameters::from_value(parameters.value())
            .map_err(|error| ToolInvokeError::InvalidInput(error.to_string()))?;
        record(&format!("{};", parameters.label));
        underlying
            .invoke_forwarding_outputs(command_path, input, stdin, stdout, stderr)
            .await
    })
}

golem_rust::ctor::__support::ctor_parse!(
    #[ctor]
    fn register_universal() {
        golem_rust::tool::register_tool_middleware(
            ToolMiddleware {
                name: "manifest-universal".to_string(),
                version: "1.0.0".to_string(),
                aliases: Vec::new(),
                doc: Default::default(),
                scope: ToolMiddlewareScope::Universal,
                parameter_schema: golem_rust::schema::try_into_schema_graph::<LabelParameters>()
                    .unwrap(),
            },
            invoke_universal,
        );
    }
);

pub mod expected_compat {
    use super::*;

    #[derive(Clone, Debug, IntoSchema, FromSchema, IntoWire, FromWire, WireSchema)]
    pub struct CompatInput {
        pub kept: String,
    }

    #[derive(Clone, Debug, IntoSchema, FromSchema, IntoWire, FromWire, WireSchema)]
    pub struct CompatOutput {
        pub kept: String,
        pub leaf_only: u64,
    }

    #[derive(Debug, Clone, ToolError)]
    pub enum CompatError {
        #[tool_error(kind = "usage-error", exit_code = 21)]
        Rejected(CompatFailure),
        #[tool_error(kind = "runtime-error", exit_code = 22)]
        MiddlewareOnly(CompatFailure),
    }

    #[derive(Clone, Debug, IntoSchema, FromSchema, IntoWire, FromWire, WireSchema)]
    pub struct CompatFailure {
        pub code: u32,
        pub leaf_only: String,
    }

    #[tool_definition(version = "1.0.0")]
    pub trait CompatLeaf {
        async fn execute(&self, input: CompatInput) -> Result<CompatOutput, CompatError>;
    }
}

pub mod presented_compat {
    pub use super::expected_compat::{CompatError, CompatInput, CompatOutput};
    use super::*;

    #[tool_definition(version = "1.0.0")]
    pub trait PresentedAdapter {
        async fn execute(&self, input: CompatInput) -> Result<CompatOutput, CompatError>;
    }
}

struct StructuralAdapter;

impl StructuralAdapter {
    fn new() -> Self {
        Self
    }
}

#[tool_middleware(name = "structural-adapter", constructor = StructuralAdapter::new)]
impl presented_compat::PresentedAdapterMiddleware<expected_compat::CompatLeafUnderlying>
    for StructuralAdapter
{
    async fn execute(
        &self,
        underlying: &expected_compat::CompatLeafUnderlying,
        input: presented_compat::CompatInput,
    ) -> Result<presented_compat::CompatOutput, ToolInvokeError<presented_compat::CompatError>>
    {
        underlying.execute(input).await
    }
}

pub mod projected_compat {
    use super::*;

    #[derive(Clone, Debug, IntoSchema, FromSchema, IntoWire, FromWire, WireSchema)]
    pub struct CompatInput {
        pub kept: String,
        pub discarded: String,
    }

    #[derive(Clone, Debug, IntoSchema, FromSchema, IntoWire, FromWire, WireSchema)]
    pub struct CompatOutput {
        pub kept: String,
    }

    #[derive(Debug, Clone, ToolError)]
    pub enum CompatError {
        #[tool_error(kind = "usage-error", exit_code = 21)]
        Rejected(CompatFailure),
    }

    #[derive(Clone, Debug, IntoSchema, FromSchema, IntoWire, FromWire, WireSchema)]
    pub struct CompatFailure {
        pub code: u32,
    }

    #[tool_definition(version = "1.0.0")]
    pub trait CompatLeaf {
        async fn execute(&self, input: CompatInput) -> Result<CompatOutput, CompatError>;
    }
}

struct StructuralProjection;

impl StructuralProjection {
    fn new() -> Self {
        Self
    }
}

#[tool_middleware(
    name = "structural-projection",
    constructor = StructuralProjection::new
)]
impl projected_compat::CompatLeafMiddleware for StructuralProjection {
    async fn execute(
        &self,
        underlying: &projected_compat::CompatLeafUnderlying,
        input: projected_compat::CompatInput,
    ) -> Result<projected_compat::CompatOutput, ToolInvokeError<projected_compat::CompatError>>
    {
        underlying.execute(input).await
    }
}

pub mod expected_nominal {
    use super::*;

    #[derive(Clone, Debug, IntoSchema, FromSchema, IntoWire, FromWire, WireSchema)]
    pub struct Payload {
        pub middleware_value: String,
    }

    #[tool_definition(version = "1.0.0")]
    pub trait NominalLeaf {
        async fn check(&self, payload: Payload) -> String;
    }
}

struct NominalAdapter;

impl NominalAdapter {
    fn new() -> Self {
        Self
    }
}

#[tool_middleware(name = "nominal-adapter", constructor = NominalAdapter::new)]
impl expected_nominal::NominalLeafMiddleware for NominalAdapter {
    async fn check(
        &self,
        underlying: &expected_nominal::NominalLeafUnderlying,
        payload: expected_nominal::Payload,
    ) -> Result<String, ToolInvokeError<std::convert::Infallible>> {
        underlying.check(payload).await
    }
}
