mod parse;
mod naming;
mod render;

pub mod model {
	#[derive(Debug, Clone)]
	pub struct WitOutput {
		pub package: String,
		pub version: String,
		pub wit_text: String,
	}
}

#[derive(thiserror::Error, Debug)]
pub enum GeneratorError {
	#[error("invalid proto: {0}")]
	Invalid(String),
}

/// Converts a proto3 gRPC source snippet into WIT text and metadata (demo scope).
pub fn convert_protobuf_to_wit(proto_src: &str) -> Result<model::WitOutput, GeneratorError> {
	let pkg = parse::parse_proto_package(proto_src).ok_or_else(|| GeneratorError::Invalid("missing package".into()))?;
	let version = "1.0.0".to_string();

	// Parse messages and service RPCs (demo scope)
	let messages = parse::parse_messages(proto_src);
	let service = parse::parse_service(proto_src);

	let header = render::WitPackage::from_proto_package(&pkg, &version).header();
	let mut body = String::new();
	for m in &messages {
		body.push_str(&render::render_message_record(m));
	}
	if let Some(svc) = service {
		body.push_str(&render::render_error_variant());
		body.push_str(&render::render_service_interface(&svc));
	}

	Ok(model::WitOutput { package: render::WitPackage::from_proto_package(&pkg, &version).name, version, wit_text: format!("{}{}", header, body) })
}
