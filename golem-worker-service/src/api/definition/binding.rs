use golem_worker_service_base::gateway_api_definition::binding as base;
use serde::{Deserialize, Serialize};
use std::convert::TryFrom;

/// Extended binding types for the worker service
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum BindingType {
    #[serde(rename = "Default")]
    Default {
        input_type: String,
        output_type: String,
        function_name: String,
    },
    #[serde(rename = "FileServer")]
    FileServer {
        root_dir: String,
    },
    /// Swagger UI binding for API documentation
    #[serde(rename = "SwaggerUI")]
    SwaggerUI {
        spec_path: String,
    },
}

impl From<base::BindingType> for BindingType {
    fn from(binding: base::BindingType) -> Self {
        match binding {
            base::BindingType::Default { input_type, output_type, function_name } => 
                BindingType::Default { input_type, output_type, function_name },
            base::BindingType::FileServer { root_dir } => 
                BindingType::FileServer { root_dir },
        }
    }
}

impl TryFrom<BindingType> for base::BindingType {
    type Error = String;

    fn try_from(binding: BindingType) -> Result<Self, Self::Error> {
        match binding {
            BindingType::Default { input_type, output_type, function_name } => 
                Ok(base::BindingType::Default { input_type, output_type, function_name }),
            BindingType::FileServer { root_dir } => 
                Ok(base::BindingType::FileServer { root_dir }),
            BindingType::SwaggerUI { .. } => 
                Err("SwaggerUI bindings cannot be converted to base binding type".to_string()),
        }
    }
}
