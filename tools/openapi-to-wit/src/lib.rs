mod naming;
mod render;

pub mod model {
    #[derive(Debug, Clone)]
    pub struct WitOutput {
        pub package: String,
        pub version: String,
        pub source_digest: String,
        pub wit_text: String,
    }
}

#[derive(thiserror::Error, Debug)]
pub enum GeneratorError {
    #[error("unsupported feature: {0}")]
    Unsupported(String),
    #[error("invalid schema: {0}")]
    Invalid(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

/// Converts an OpenAPI 3.0.x document (YAML/JSON text) into WIT text and metadata.
pub fn convert_openapi_to_wit(openapi_text: &str) -> Result<model::WitOutput, GeneratorError> {
    // TODO: parse openapi_text (YAML/JSON) and derive package/version
    let package = "api:todos".to_string();
    let version = "1.0.0".to_string();

    let header = render::WitPackage { name: package.clone(), version: version.clone() }.render_header();

    Ok(model::WitOutput {
        package,
        version,
        source_digest: format!("sha256:{}", blake3::hash(openapi_text.as_bytes())),
        wit_text: header,
    })
}
