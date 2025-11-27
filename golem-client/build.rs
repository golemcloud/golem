use std::env::var_os;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use golem_openapi_client_generator::parse_openapi_specs;

fn main() {
    let out_dir = var_os("OUT_DIR").unwrap();

    let root_yaml_path = PathBuf::from("../openapi/golem-service.yaml");
    let local_yaml_path = PathBuf::from("openapi/golem-service.yaml");

    println!("cargo::rerun-if-changed={}", root_yaml_path.display());
    println!("cargo::rerun-if-changed={}", local_yaml_path.display());

    println!("Starting code generation for Golem OpenAPI client.");

    println!("Output directory: {out_dir:?}");
    println!("Workspace OpenAPI file: {root_yaml_path:?}");

    if root_yaml_path.exists() {
        // Copying the file to the crate so it gets packaged
        std::fs::create_dir_all(local_yaml_path.parent().unwrap()).unwrap();
        copy_if_different(root_yaml_path.clone(), local_yaml_path.clone()).unwrap();
    };
    generate(local_yaml_path.clone(), out_dir)
}

fn generate(yaml_path: PathBuf, out_dir: OsString) {
    golem_openapi_client_generator::gen(
        parse_openapi_specs(&[yaml_path]).expect("Failed to parse OpenAPI spec."),
        Path::new(&out_dir),
        "golem-client",
        "0.0.0",
        false,
        true,
        // To keep this more organized, group the mappings here by module in ::golem_common::model
        &[
            // Account
            ("Account", "golem_common::model::account::Account"),
            (
                "AccountCreation",
                "golem_common::model::account::AccountCreation",
            ),
            (
                "AccountUpdate",
                "golem_common::model::account::AccountUpdate",
            ),
            (
                "AccountSetRoles",
                "golem_common::model::account::AccountSetRoles",
            ),
            ("Plan", "golem_common::model::account::Plan"),
            // Application
            (
                "Application",
                "golem_common::model::application::Application",
            ),
            (
                "ApplicationCreation",
                "golem_common::model::application::ApplicationCreation",
            ),
            (
                "ApplicationUpdate",
                "golem_common::model::application::ApplicationUpdate",
            ),
            // Auth
            ("Token", "golem_common::model::auth::Token"),
            ("TokenCreation", "golem_common::model::auth::TokenCreation"),
            (
                "TokenWithSecret",
                "golem_common::model::auth::TokenWithSecret",
            ),
            ("AccountRole", "golem_common::model::auth::AccountRole"),
            (
                "EnvironmentRole",
                "golem_common::model::auth::EnvironmentRole",
            ),
            // Component
            (
                "ComponentCreation",
                "golem_common::model::component::ComponentCreation",
            ),
            (
                "ComponentUpdate",
                "golem_common::model::component::ComponentUpdate",
            ),
            (
                "ComponentDto",
                "golem_common::model::component::ComponentDto",
            ),
            (
                "ComponentFileOptions",
                "golem_common::model::component::ComponentFileOptions",
            ),
            (
                "InstalledPlugin",
                "golem_common::model::component::InstalledPlugin",
            ),
            (
                "PluginInstallation",
                "golem_common::model::component::PluginInstallation",
            ),
            (
                "PluginInstallationUpdate",
                "golem_common::model::component::PluginInstallationUpdate",
            ),
            (
                "PluginUninstallation",
                "golem_common::model::component::PluginUninstallation",
            ),
            // Component Metadata
            (
                "ComponentMetadata",
                "golem_common::model::component_metadata::ComponentMetadata",
            ),
            // Deployment
            ("Deployment", "golem_common::model::deployment::Deployment"),
            (
                "DeploymentCreation",
                "golem_common::model::deployment::DeploymentCreation",
            ),
            (
                "DeploymentPlan",
                "golem_common::model::deployment::DeploymentPlan",
            ),
            (
                "DeploymentPlanComponentEntry",
                "golem_common::model::deployment::DeploymentPlanComponentEntry",
            ),
            (
                "DeploymentSummary",
                "golem_common::model::deployment::DeploymentSummary",
            ),
            // Domain Registration
            (
                "DomainRegistrationCreation",
                "golem_common::model::domain_registration::DomainRegistrationCreation",
            ),
            (
                "DomainRegistration",
                "golem_common::model::domain_registration::DomainRegistration",
            ),
            // Environment
            (
                "Environment",
                "golem_common::model::environment::Environment",
            ),
            (
                "EnvironmentCreation",
                "golem_common::model::environment::EnvironmentCreation",
            ),
            (
                "EnvironmentUpdate",
                "golem_common::model::environment::EnvironmentUpdate",
            ),
            // Environment Plugin Grant
            (
                "EnvironmentPluginGrant",
                "golem_common::model::environment_plugin_grant::EnvironmentPluginGrant",
            ),
            (
                "EnvironmentPluginGrantCreation",
                "golem_common::model::environment_plugin_grant::EnvironmentPluginGrantCreation",
            ),
            // Environment Share
            (
                "EnvironmentShare",
                "golem_common::model::environment_share::EnvironmentShare",
            ),
            (
                "EnvironmentShareCreation",
                "golem_common::model::environment_share::EnvironmentShareCreation",
            ),
            (
                "EnvironmentShareUpdate",
                "golem_common::model::environment_share::EnvironmentShareUpdate",
            ),
            // Plugin Registration
            (
                "PluginRegistrationDto",
                "golem_common::model::plugin_registration::PluginRegistrationDto",
            ),
            (
                "PluginRegistrationCreation",
                "golem_common::model::plugin_registration::PluginRegistrationCreation",
            ),
            (
                "ComponentTransformerPluginSpec",
                "golem_common::model::plugin_registration::ComponentTransformerPluginSpec",
            ),
            (
                "OplogProcessorPluginSpec",
                "golem_common::model::plugin_registration::OplogProcessorPluginSpec",
            ),
            // Security Scheme
            (
                "SecuritySchemeCreation",
                "golem_common::model::security_scheme::SecuritySchemeCreation",
            ),
            (
                "SecuritySchemeUpdate",
                "golem_common::model::security_scheme::SecuritySchemeUpdate",
            ),
            (
                "SecuritySchemeDto",
                "golem_common::model::security_scheme::SecuritySchemeDto",
            ),
            // Worker
            (
                "FlatComponentFileSystemNode",
                "golem_common::model::worker::FlatComponentFileSystemNode",
            ),
            (
                "RevertWorkerTarget",
                "golem_common::model::worker::RevertWorkerTarget",
            ),
            ("ScanCursor", "golem_common::model::ScanCursor"),
            (
                "WorkerMetadataDto",
                "golem_common::model::worker::WorkerMetadataDto",
            ),
            (
                "WorkerUpdateMode",
                "golem_common::model::worker::WorkerUpdateMode",
            ),
            // Public Oplog
            (
                "PublicOplogEntryWithIndex",
                "golem_common::model::oplog::PublicOplogEntryWithIndex",
            ),
            // Http api definition
            (
                "HttpApiDefinitionCreation",
                "golem_common::model::http_api_definition::HttpApiDefinitionCreation",
            ),
            (
                "HttpApiDefinitionUpdate",
                "golem_common::model::http_api_definition::HttpApiDefinitionUpdate",
            ),
            (
                "HttpApiDefinition",
                "golem_common::model::http_api_definition::HttpApiDefinition",
            ),
            // Http api deployment
            (
                "HttpApiDeploymentCreation",
                "golem_common::model::http_api_deployment::HttpApiDeploymentCreation",
            ),
            (
                "HttpApiDeploymentUpdate",
                "golem_common::model::http_api_deployment::HttpApiDeploymentUpdate",
            ),
            (
                "HttpApiDeployment",
                "golem_common::model::http_api_deployment::HttpApiDeployment",
            ),
            // TODO: Leftovers
            ("PluginScope", "golem_common::model::plugin::PluginScope"),
            (
                "RegisteredAgentType",
                "golem_common::model::agent::RegisteredAgentType",
            ),
            ("ShardId", "golem_common::model::ShardId"),
            ("ValueAndType", "golem_wasm_rpc::ValueAndType"),
            ("AgentType", "golem_common::model::agent::AgentType"),
            ("DataSchema", "golem_common::model::agent::DataSchema"),
            ("AgentInstanceKey", "golem_common::model::AgentInstanceKey"),
            (
                "AgentInstanceDescription",
                "golem_common::model::AgentInstanceDescription",
            ),
            ("AnalysedExport", "golem_wasm::analysis::AnalysedExport"),
            ("AnalysedType", "golem_wasm::analysis::AnalysedType"),
            ("PluginScope", "golem_common::model::plugin::PluginScope"),
            (
                "ComponentMetadata",
                "golem_common::model::component_metadata::ComponentMetadata",
            ),
            (
                "ComponentFilePathWithPermissionsList",
                "golem_common::model::ComponentFilePathWithPermissionsList",
            ),
            ("ComponentType", "golem_common::model::ComponentType"),
            ("DataValue", "golem_common::model::agent::DataValue"),
            ("Empty", "golem_common::model::Empty"),
            (
                "InitialComponentFile",
                "golem_common::model::InitialComponentFile",
            ),
            ("ErrorBody", "golem_common::model::error::ErrorBody"),
            ("ErrorsBody", "golem_common::model::error::ErrorsBody"),
            (
                "ExportedResourceInstanceKey",
                "golem_common::model::ExportedResourceInstanceKey",
            ),
            (
                "ExportedResourceInstanceDescription",
                "golem_common::model::ExportedResourceInstanceDescription",
            ),
            ("GolemError", "golem_common::model::error::GolemError"),
            (
                "PluginInstallationAction",
                "golem_common::model::plugin::PluginInstallationAction",
            ),
            ("OplogCursor", "golem_common::model::oplog::OplogCursor"),
            ("OplogRegion", "golem_common::model::regions::OplogRegion"),
            (
                "ProjectActions",
                "golem_common::model::auth::ProjectActions",
            ),
            (
                "ProjectPermission",
                "golem_common::model::auth::ProjectPermission",
            ),
            ("PromiseId", "golem_common::model::PromiseId"),
            (
                "PublicOplogEntry",
                "golem_common::model::oplog::PublicOplogEntry",
            ),
            (
                "RegisteredAgentType",
                "golem_common::model::agent::RegisteredAgentType",
            ),
            ("ShardId", "golem_common::model::ShardId"),
            ("ValueAndType", "golem_wasm::ValueAndType"),
            (
                "ValueAndOptionalType",
                "golem_wasm::json::OptionallyValueAndTypeJson",
            ),
            (
                "WasiConfigVarsEntry",
                "golem_common::model::worker::WasiConfigVarsEntry",
            ),
            (
                "WasmRpcTarget",
                "golem_common::model::component_metadata::WasmRpcTarget",
            ),
            (
                "WorkerCreationRequest",
                "golem_common::model::worker::WorkerCreationRequest",
            ),
            ("WorkerFilter", "golem_common::model::WorkerFilter"),
            ("WorkerId", "golem_common::model::WorkerId"),
            (
                "WorkerBindingType",
                "golem_common::model::WorkerBindingType",
            ),
            (
                "WorkerResourceDescription",
                "golem_common::model::WorkerResourceDescription",
            ),
            ("WorkerStatus", "golem_common::model::WorkerStatus"),
        ],
        &["/v1/components/{component_id}/workers/{worker_name}/connect"],
    )
    .expect("Failed to generate client code from OpenAPI spec.");
}

fn copy_if_different(
    src: impl AsRef<Path> + Sized,
    dst: impl AsRef<Path> + Sized,
) -> std::io::Result<()> {
    if dst.as_ref().exists() {
        let a = std::fs::read(&src)?;
        let b = std::fs::read(&dst)?;
        if a != b {
            std::fs::copy(src, dst)?;
        }
        Ok(())
    } else {
        std::fs::copy(src, dst)?;
        Ok(())
    }
}
