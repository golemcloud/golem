use golem_rust::{
    FromSchema, FromWire, IntoSchema, IntoWire, ToolError, WireSchema, tool_definition,
    tool_implementation,
};
use std::io::Write;

fn record(marker: &str) {
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("/chunk-d-effects.log")
        .and_then(|mut file| file.write_all(marker.as_bytes()))
        .expect("the leaf has owner filesystem access");
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

struct ManifestProbeImpl;

#[tool_implementation]
impl ManifestProbe for ManifestProbeImpl {
    async fn apply(&self, value: String) -> Result<String, ProbeError> {
        record(&format!("leaf:{value};"));
        if value.starts_with("reject(") {
            Err(ProbeError::Rejected { value })
        } else {
            Ok(format!("leaf({value})"))
        }
    }
}

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

struct CompatLeafImpl;

#[tool_implementation]
impl CompatLeaf for CompatLeafImpl {
    async fn execute(&self, input: CompatInput) -> Result<CompatOutput, CompatError> {
        record(&format!("compat:{};", input.kept));
        if input.kept == "reject" {
            Err(CompatError::Rejected(CompatFailure {
                code: 23,
                leaf_only: "private-detail".to_string(),
            }))
        } else {
            Ok(CompatOutput {
                kept: format!("leaf: {}", input.kept),
                leaf_only: 99,
            })
        }
    }
}

pub mod nominal {
    use super::*;

    #[derive(Clone, Debug, IntoSchema, FromSchema, IntoWire, FromWire, WireSchema)]
    pub struct Payload {
        pub leaf_value: String,
    }

    #[tool_definition(version = "1.0.0")]
    pub trait NominalLeaf {
        async fn check(&self, payload: Payload) -> String;
    }

    pub struct NominalLeafImpl;

    #[tool_implementation]
    impl NominalLeaf for NominalLeafImpl {
        async fn check(&self, payload: Payload) -> String {
            super::record(&format!("nominal:{};", payload.leaf_value));
            payload.leaf_value
        }
    }
}
