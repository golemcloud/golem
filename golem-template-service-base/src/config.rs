use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "type", content = "config")]
#[derive(Default)]
pub enum TemplateCompilationConfig {
    Enabled(TemplateCompilationEnabledConfig),
    #[default]
    Disabled,
}

#[derive(Clone, Debug, Deserialize)]
pub struct TemplateCompilationEnabledConfig {
    pub host: String,
    pub port: u16,
}

impl TemplateCompilationEnabledConfig {
    pub fn uri(&self) -> http_02::Uri {
        http_02::Uri::builder()
            .scheme("http")
            .authority(format!("{}:{}", self.host, self.port).as_str())
            .path_and_query("/")
            .build()
            .expect("Failed to build TemplateCompilationService URI")
    }
}
