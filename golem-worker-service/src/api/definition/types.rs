use serde::{Deserialize, Serialize};
use golem_wasm_ast::analysis::AnalysedType;

/// Base binding types for the API Gateway
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum BindingType {
    Http,
    Worker,
    Proxy,
    #[serde(rename = "Default")]
    Default {
        input_type: AnalysedType,
        output_type: AnalysedType,
        function_name: String,
    },
    #[serde(rename = "FileServer")]
    FileServer {
        root_dir: String,
    },
    #[serde(rename = "SwaggerUI")]
    SwaggerUI {
        doc_url: String,
    },
}

impl std::fmt::Display for BindingType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BindingType::Default { input_type, output_type, function_name } => {
                write!(f, "Default({:?}, {:?}, {})", input_type, output_type, function_name)
            },
            BindingType::FileServer { root_dir } => {
                write!(f, "FileServer({})", root_dir)
            },
            BindingType::SwaggerUI { doc_url } => {
                write!(f, "SwaggerUI({})", doc_url)
            },
            _ => write!(f, "{:?}", self),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiDefinition {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub routes: Vec<Route>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Route {
    pub path: String,
    pub method: HttpMethod,
    pub description: String,
    pub component_name: String,
    pub binding: BindingType,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Delete,
    Patch,
    Head,
    Options,
}

impl std::fmt::Display for HttpMethod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HttpMethod::Get => write!(f, "GET"),
            HttpMethod::Post => write!(f, "POST"),
            HttpMethod::Put => write!(f, "PUT"),
            HttpMethod::Delete => write!(f, "DELETE"),
            HttpMethod::Patch => write!(f, "PATCH"),
            HttpMethod::Head => write!(f, "HEAD"),
            HttpMethod::Options => write!(f, "OPTIONS"),
        }
    }
}