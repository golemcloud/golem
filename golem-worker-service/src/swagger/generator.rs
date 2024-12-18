use std::path::{Path, PathBuf};
use tokio::fs;
use anyhow::Result;

pub struct SwaggerGenerator {
    output_dir: PathBuf,
}

impl SwaggerGenerator {
    pub fn new<P: AsRef<Path>>(output_dir: P) -> Self {
        Self {
            output_dir: output_dir.as_ref().to_path_buf(),
        }
    }

    pub async fn generate(&self, api_id: &str, spec_path: &str) -> Result<()> {
        // Create directory for this API's Swagger UI
        let api_dir = self.output_dir.join(api_id);
        fs::create_dir_all(&api_dir).await?;

        // Copy Swagger UI static files
        self.copy_swagger_assets(&api_dir).await?;

        // Generate customized index.html
        let index_html = self.generate_index_html(spec_path);
        fs::write(api_dir.join("index.html"), index_html).await?;

        Ok(())
    }

    async fn copy_swagger_assets(&self, target_dir: &Path) -> Result<()> {
        // Copy embedded Swagger UI assets
        let assets = [
            "swagger-ui.css",
            "swagger-ui-bundle.js",
            "swagger-ui-standalone-preset.js",
            "favicon-32x32.png",
            "favicon-16x16.png",
        ];

        for asset in assets {
            let content = include_bytes!(concat!("../../assets/swagger-ui/", asset));
            fs::write(target_dir.join(asset), content).await?;
        }

        Ok(())
    }

    fn generate_index_html(&self, spec_path: &str) -> String {
        format!(
            r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <title>Swagger UI</title>
    <link rel="stylesheet" type="text/css" href="./swagger-ui.css" />
    <link rel="icon" type="image/png" href="./favicon-32x32.png" sizes="32x32" />
    <link rel="icon" type="image/png" href="./favicon-16x16.png" sizes="16x16" />
</head>
<body>
    <div id="swagger-ui"></div>
    <script src="./swagger-ui-bundle.js"></script>
    <script src="./swagger-ui-standalone-preset.js"></script>
    <script>
    window.onload = function() {{
        window.ui = SwaggerUIBundle({{
            url: "{}",
            dom_id: '#swagger-ui',
            deepLinking: true,
            presets: [
                SwaggerUIBundle.presets.apis,
                SwaggerUIStandalonePreset
            ],
            plugins: [
                SwaggerUIBundle.plugins.DownloadUrl
            ],
        }});
    }};
    </script>
</body>
</html>"#,
            spec_path
        )
    }

    pub async fn clean(&self, api_id: &str) -> Result<()> {
        let api_dir = self.output_dir.join(api_id);
        if api_dir.exists() {
            fs::remove_dir_all(api_dir).await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_swagger_generator() {
        let temp_dir = tempdir().unwrap();
        let generator = SwaggerGenerator::new(temp_dir.path());

        generator.generate("test-api", "/api/spec/test").await.unwrap();

        // Verify files were created
        assert!(temp_dir.path().join("test-api/index.html").exists());
        assert!(temp_dir.path().join("test-api/swagger-ui.css").exists());
        assert!(temp_dir.path().join("test-api/swagger-ui-bundle.js").exists());

        // Clean up
        generator.clean("test-api").await.unwrap();
        assert!(!temp_dir.path().join("test-api").exists());
    }
}
