pub use crate::base_model::tool_middleware_release::*;

use crate::model::diff;
use crate::schema::tool::ToolMiddleware;

pub fn tool_middleware_metadata_digest(
    metadata_version: &str,
    definition: &ToolMiddleware,
) -> anyhow::Result<diff::Hash> {
    let mut input = Vec::from(b"golem:tool-middleware-metadata:v1\0".as_slice());
    input.extend_from_slice(metadata_version.as_bytes());
    input.push(0);
    input.extend_from_slice(&desert_rust::serialize_to_byte_vec(definition)?);
    Ok(blake3::hash(&input).into())
}
