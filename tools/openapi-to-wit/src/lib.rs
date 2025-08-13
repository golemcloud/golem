mod naming;
mod render;
mod parse;

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
    // Parse header info
    let (title, version) = parse::parse_title_version(openapi_text)
        .ok_or_else(|| GeneratorError::Invalid("missing or invalid info section".into()))?;
    let pkg_name = naming::to_wit_ident(&title);

    // Header
    let header = render::WitPackage { name: format!("api:{}", pkg_name), version: version.clone() }.render_header();

    // Records from components.schemas
    let records = parse::parse_component_records(openapi_text);
    let mut body = String::new();
    for rec in &records {
        body.push_str(&render::render_record(rec));
    }

    // Operations -> single interface named from title
    let ops = parse::parse_operations(openapi_text);
    if !ops.is_empty() {
        body.push_str(&render::render_error_variant());
        body.push_str(&render::render_interface(&pkg_name, &ops));
    }

    Ok(model::WitOutput {
        package: format!("api:{}", pkg_name),
        version,
        source_digest: format!("sha256:{}", blake3::hash(openapi_text.as_bytes())),
        wit_text: format!("{}{}", header, body),
    })
}
