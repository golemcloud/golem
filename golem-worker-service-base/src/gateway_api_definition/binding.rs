use serde::{Deserialize, Serialize};

/// Base binding types for the API Gateway
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum BindingType {
    /// Function binding that calls into worker
    Default {
        input_type: String,
        output_type: String,
        function_name: String,
    },
    /// Static file server binding
    FileServer {
        root_dir: String,
    },
}
