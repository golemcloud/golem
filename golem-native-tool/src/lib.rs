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

pub trait NativeToolStdinHandle: Send {
    fn read<'a>(&'a mut self) -> NativeToolReadFuture<'a>;
}

pub type NativeToolReadFuture<'a> =
    Pin<Box<dyn Future<Output = Option<Result<Vec<u8>, String>>> + Send + 'a>>;

pub trait NativeToolStdoutHandle: Send {
    fn write<'a>(
        &'a mut self,
        bytes: Vec<u8>,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>>;
    fn finish(&mut self) -> Result<(), String>;
}

pub type NativeToolStdin = Box<dyn NativeToolStdinHandle>;
pub type NativeToolStdout = Box<dyn NativeToolStdoutHandle>;

pub struct NativeToolInvocation {
    pub command_path: Vec<String>,
    pub input: TypedSchemaValue,
    pub principal: Principal,
    pub stdin: Option<NativeToolStdin>,
    pub stdout: Option<NativeToolStdout>,
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
    Custom(TypedSchemaValue),
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
        invocation: NativeToolInvocation,
    ) -> NativeToolFuture<'a, E>;
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
