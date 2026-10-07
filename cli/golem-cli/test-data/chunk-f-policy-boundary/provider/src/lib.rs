use golem_rust::golem_agentic::golem::agent::host as agent_host;
use golem_rust::{
    FromSchema, decode_schema_value, encode_schema_graph, tool_definition, tool_implementation,
};

#[tool_definition(version = "1.0.0")]
pub trait ChunkFConfigProbe {
    async fn read(&self, key: String) -> String;
}

struct ChunkFConfigProbeImpl;

#[tool_implementation]
impl ChunkFConfigProbe for ChunkFConfigProbeImpl {
    async fn read(&self, key: String) -> String {
        let graph = golem_rust::schema::try_into_schema_graph::<Option<String>>().unwrap();
        let expected = encode_schema_graph(&graph).unwrap();
        match agent_host::get_config_value(&[key], &expected) {
            Ok(value) => {
                let value = decode_schema_value(value).unwrap();
                Option::<String>::from_value(&value)
                    .unwrap()
                    .unwrap_or_else(|| "missing".to_string())
            }
            Err(_) => "denied".to_string(),
        }
    }
}

#[tool_definition(version = "1.0.0", requires_filesystem = true)]
pub trait ChunkFFilesystemProbe {
    async fn roundtrip(&self, path: String, value: String) -> String;
}

struct ChunkFFilesystemProbeImpl;

#[tool_implementation]
impl ChunkFFilesystemProbe for ChunkFFilesystemProbeImpl {
    async fn roundtrip(&self, path: String, value: String) -> String {
        std::fs::write(&path, value).unwrap();
        std::fs::read_to_string(path).unwrap()
    }
}
