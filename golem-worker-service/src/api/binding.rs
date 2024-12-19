use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BindingType {
    // ...existing code...
    SwaggerUI {
        spec_path: String,
        theme: Option<String>,
    },
}

impl BindingType {
    pub fn create_handler(&self) -> Box<dyn BindingHandler> {
        match self {
            // ...existing code...
            BindingType::SwaggerUI { spec_path, theme } => {
                Box::new(SwaggerUIBinding::new(
                    spec_path.clone(),
                    theme.clone().unwrap_or_default(),
                ))
            }
        }
    }
}
