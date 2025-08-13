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
pub fn convert_openapi_to_wit(_openapi_text: &str) -> Result<model::WitOutput, GeneratorError> {
	// TODO: implement parsing, naming, and rendering
	Ok(model::WitOutput {
		package: "api:todos".to_string(),
		version: "1.0.0".to_string(),
		source_digest: "sha256:TODO".to_string(),
		wit_text: "// TODO".to_string(),
	})
}
