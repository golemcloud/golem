use std::path::{Path, PathBuf};
use std::fs;
use tokio::process::Command;
use thiserror::Error;
use crate::gateway_api_definition::http::openapi_export::{OpenApiExporter, OpenApiFormat};
use utoipa::openapi::OpenApi;
use url::Url;

#[derive(Debug, Error)]
pub enum ClientGenerationError {
    #[error("Failed to create directory: {0}")]
    DirectoryCreationError(#[from] std::io::Error),
    #[error("Failed to generate client: {0}")]
    GenerationError(String),
    #[error("Failed to export OpenAPI spec: {0}")]
    OpenApiExportError(String),
    #[error("Failed to convert path: {0}")]
    PathConversionError(String),
}

pub struct ClientGenerator {
    exporter: OpenApiExporter,
    output_dir: PathBuf,
}

impl ClientGenerator {
    pub fn new<P: AsRef<Path>>(output_dir: P) -> Self {
        Self {
            exporter: OpenApiExporter,
            output_dir: PathBuf::from(output_dir.as_ref().to_str().unwrap().replace('\\', "/")),
        }
    }

    fn normalize_path<P: AsRef<Path>>(&self, path: P) -> Result<String, ClientGenerationError> {
        let path_str = path.as_ref()
            .to_str()
            .ok_or_else(|| ClientGenerationError::PathConversionError("Invalid path".to_string()))?
            .replace('\\', "/");

        if cfg!(windows) {
            Ok(Url::from_file_path(&path_str)
                .map_err(|_| ClientGenerationError::PathConversionError("Failed to create URL".to_string()))?
                .to_string()
                .replace("file:///", ""))
        } else {
            Ok(path_str)
        }
    }

    async fn run_openapi_generator(
        &self,
        spec_path: &Path,
        output_dir: &Path,
        generator: &str,
        additional_properties: &str,
    ) -> Result<(), ClientGenerationError> {
        let spec_path_str = self.normalize_path(spec_path)?;
        let output_dir_str = self.normalize_path(output_dir)?;

        let args = vec![
            "generate".to_string(),
            "-i".to_string(),
            spec_path_str,
            "-g".to_string(),
            generator.to_string(),
            "-o".to_string(),
            output_dir_str,
            format!("--additional-properties={}", additional_properties),
            "--skip-validate-spec".to_string(), // Skip validation to avoid path issues
        ];

        #[cfg(windows)]
        let jar_path = format!(
            "{}/.openapi-generator/openapi-generator-cli.jar",
            std::env::var("USERPROFILE").unwrap_or_default().replace('\\', "/")
        );

        #[cfg(windows)]
        let output = Command::new("java")
            .arg("-jar")
            .arg(&jar_path)
            .args(&args)
            .output()
            .await
            .map_err(|e| ClientGenerationError::GenerationError(e.to_string()))?;

        #[cfg(not(windows))]
        let output = Command::new("openapi-generator-cli")
            .args(&args)
            .output()
            .await
            .map_err(|e| ClientGenerationError::GenerationError(e.to_string()))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            return Err(ClientGenerationError::GenerationError(format!(
                "openapi-generator failed:\nstdout: {}\nstderr: {}",
                stdout, stderr
            )));
        }

        Ok(())
    }

    pub async fn generate_rust_client(
        &self,
        api_id: &str,
        version: &str,
        openapi: OpenApi,
        package_name: &str,
    ) -> Result<PathBuf, ClientGenerationError> {
        // Create output directory with forward slashes
        let client_dir = self.output_dir.join(format!("{}-{}-rust", api_id, version));
        fs::create_dir_all(&client_dir)?;

        // Export OpenAPI spec
        let format = OpenApiFormat { json: true };
        let exported = self.exporter.export_openapi(api_id, version, openapi, &format);

        // Write OpenAPI spec to file
        let spec_path = client_dir.join("openapi.json");
        fs::write(&spec_path, &exported)?;

        // Generate Rust client
        self.run_openapi_generator(
            &spec_path,
            &client_dir,
            "rust",
            &format!("packageName={}", package_name),
        )
        .await?;

        Ok(client_dir)
    }

    pub async fn generate_typescript_client(
        &self,
        api_id: &str,
        version: &str,
        openapi: OpenApi,
        package_name: &str,
    ) -> Result<PathBuf, ClientGenerationError> {
        // Create output directory with forward slashes
        let client_dir = self.output_dir.join(format!("{}-{}-typescript", api_id, version));
        fs::create_dir_all(&client_dir)?;

        // Export OpenAPI spec
        let format = OpenApiFormat { json: true };
        let exported = self.exporter.export_openapi(api_id, version, openapi, &format);

        // Write OpenAPI spec to file
        let spec_path = client_dir.join("openapi.json");
        fs::write(&spec_path, &exported)?;

        // Generate TypeScript client
        self.run_openapi_generator(
            &spec_path,
            &client_dir,
            "typescript-fetch",
            &format!("npmName={}", package_name),
        )
        .await?;

        Ok(client_dir)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use utoipa::openapi::{Info, OpenApiVersion};
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_rust_client_generation() {
        let temp_dir = tempdir().unwrap();
        let temp_path = temp_dir.path().to_str().unwrap().replace('\\', "/");
        let generator = ClientGenerator::new(temp_path);

        // Create test OpenAPI spec
        let mut openapi = OpenApi::new(
            Info::new("Test API", "1.0.0"),
            OpenApiVersion::V3_0_3,
        );

        // Add a test endpoint
        let mut path_item = utoipa::openapi::path::PathItem::new();
        let operation = utoipa::openapi::path::OperationBuilder::new()
            .operation_id(Some("testEndpoint"))
            .description(Some("A test endpoint"))
            .response("200", utoipa::openapi::Response::new("Success"))
            .build();
        path_item.get = Some(operation);
        openapi.paths.paths.insert("/test".to_string(), path_item);

        // Generate client
        let result = generator
            .generate_rust_client("test-api", "1.0.0", openapi, "test_client")
            .await;

        assert!(result.is_ok());
        let client_dir = result.unwrap();
        assert!(client_dir.exists());
        assert!(client_dir.join("Cargo.toml").exists());
        assert!(client_dir.join("src/lib.rs").exists());
    }

    #[tokio::test]
    async fn test_typescript_client_generation() {
        let temp_dir = tempdir().unwrap();
        let temp_path = temp_dir.path().to_str().unwrap().replace('\\', "/");
        let generator = ClientGenerator::new(temp_path);

        // Create test OpenAPI spec
        let mut openapi = OpenApi::new(
            Info::new("Test API", "1.0.0"),
            OpenApiVersion::V3_0_3,
        );

        // Add a test endpoint
        let mut path_item = utoipa::openapi::path::PathItem::new();
        let operation = utoipa::openapi::path::OperationBuilder::new()
            .operation_id(Some("testEndpoint"))
            .description(Some("A test endpoint"))
            .response("200", utoipa::openapi::Response::new("Success"))
            .build();
        path_item.get = Some(operation);
        openapi.paths.paths.insert("/test".to_string(), path_item);

        // Generate client
        let result = generator
            .generate_typescript_client("test-api", "1.0.0", openapi, "@test/client")
            .await;

        assert!(result.is_ok());
        let client_dir = result.unwrap();
        assert!(client_dir.exists());
        assert!(client_dir.join("package.json").exists());
        assert!(client_dir.join("api.ts").exists());
    }
} 