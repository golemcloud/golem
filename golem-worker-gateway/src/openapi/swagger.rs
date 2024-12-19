use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;
use serde::{Deserialize, Serialize};
use crate::error::Result;
use crate::openapi::converter::ApiDefinitionConverter;
use crate::api::ApiDefinition;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwaggerUIConfig {
    pub title: String,
    pub description: Option<String>,
    pub version: String,
    pub theme: SwaggerUITheme,
    pub default_models_expand_depth: i32,
    pub default_model_expand_depth: i32,
    pub default_model_rendering: ModelRendering,
    pub display_request_duration: bool,
    pub doc_expansion: DocExpansion,
    pub filter: bool,
    pub persist_authorization: bool,
    pub syntax_highlight: SyntaxHighlight,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SwaggerUITheme {
    Default,
    Dark,
    Feeling,
    Material,
    Monokai,
    Muted,
    Newspaper,
    Outline,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ModelRendering {
    Example,
    Model,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DocExpansion {
    List,
    Full,
    None,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SyntaxHighlight {
    Agate,
    Arta,
    Monokai,
    Nord,
    ObsidianCustom,
}

impl Default for SwaggerUIConfig {
    fn default() -> Self {
        Self {
            title: "API Documentation".to_string(),
            description: None,
            version: "1.0.0".to_string(),
            theme: SwaggerUITheme::Default,
            default_models_expand_depth: 1,
            default_model_expand_depth: 1,
            default_model_rendering: ModelRendering::Example,
            display_request_duration: true,
            doc_expansion: DocExpansion::List,
            filter: true,
            persist_authorization: true,
            syntax_highlight: SyntaxHighlight::Monokai,
        }
    }
}

pub struct SwaggerUIAssets {
    html: String,
    spec: String,
    config: SwaggerUIConfig,
    last_modified: chrono::DateTime<chrono::Utc>,
}

impl SwaggerUIAssets {
    pub fn new(spec: String, config: SwaggerUIConfig) -> Self {
        let html = Self::generate_html(&spec, &config);
        Self {
            html,
            spec,
            config,
            last_modified: chrono::Utc::now(),
        }
    }

    fn generate_html(spec: &str, config: &SwaggerUIConfig) -> String {
        let mut html = include_str!("../templates/swagger-ui.html").to_string();
        
        // Replace template variables
        html = html.replace("{{TITLE}}", &config.title);
        html = html.replace("{{THEME}}", &format!("{:?}", config.theme).to_lowercase());
        html = html.replace("{{SPEC_URL}}", "./swagger.json");
        html = html.replace("{{CONFIG}}", &serde_json::to_string(config).unwrap());
        
        html
    }

    pub fn get_html(&self) -> &str {
        &self.html
    }

    pub fn get_spec(&self) -> &str {
        &self.spec
    }

    pub fn get_last_modified(&self) -> chrono::DateTime<chrono::Utc> {
        self.last_modified
    }
}

pub struct SwaggerUIStore {
    assets: Arc<RwLock<HashMap<String, SwaggerUIAssets>>>,
    base_path: PathBuf,
}

impl SwaggerUIStore {
    pub fn new<P: AsRef<Path>>(base_path: P) -> Self {
        Self {
            assets: Arc::new(RwLock::new(HashMap::new())),
            base_path: base_path.as_ref().to_path_buf(),
        }
    }

    pub async fn store_assets(&self, api_id: &str, version: &str, assets: SwaggerUIAssets) -> Result<()> {
        let mut store = self.assets.write().await;
        let key = format!("{}/{}", api_id, version);
        store.insert(key.clone(), assets);

        // Persist to disk for durability
        let spec_path = self.base_path.join(&key).join("swagger.json");
        let html_path = self.base_path.join(&key).join("index.html");
        
        tokio::fs::create_dir_all(self.base_path.join(&key)).await?;
        tokio::fs::write(&spec_path, store.get(&key).unwrap().get_spec()).await?;
        tokio::fs::write(&html_path, store.get(&key).unwrap().get_html()).await?;
        
        Ok(())
    }

    pub async fn get_assets(&self, api_id: &str, version: &str) -> Option<SwaggerUIAssets> {
        let store = self.assets.read().await;
        let key = format!("{}/{}", api_id, version);
        store.get(&key).cloned()
    }

    pub async fn update_assets(&self, api_id: &str, version: &str, api_def: &ApiDefinition) -> Result<()> {
        let converter = ApiDefinitionConverter::new();
        let openapi = converter.convert(api_def)?;
        let spec = serde_json::to_string_pretty(&openapi)?;
        
        let config = SwaggerUIConfig {
            title: format!("{} API Documentation", api_id),
            description: Some(format!("API Documentation for {} version {}", api_id, version)),
            version: version.to_string(),
            ..Default::default()
        };
        
        let assets = SwaggerUIAssets::new(spec, config);
        self.store_assets(api_id, version, assets).await?;
        
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_swagger_ui_store() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let store = SwaggerUIStore::new(temp_dir.path());

        let api_id = "test-api";
        let version = "1.0.0";
        let spec = r#"{"openapi":"3.0.0","info":{"title":"Test API","version":"1.0.0"}}"#.to_string();
        let config = SwaggerUIConfig::default();
        let assets = SwaggerUIAssets::new(spec.clone(), config);

        // Test storing assets
        store.store_assets(api_id, version, assets).await?;

        // Test retrieving assets
        let retrieved = store.get_assets(api_id, version).await.unwrap();
        assert_eq!(retrieved.get_spec(), spec);

        // Verify files were created
        let spec_path = temp_dir.path().join(format!("{}/{}/swagger.json", api_id, version));
        let html_path = temp_dir.path().join(format!("{}/{}/index.html", api_id, version));
        assert!(spec_path.exists());
        assert!(html_path.exists());

        Ok(())
    }
}
