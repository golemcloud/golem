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
        &[
            // account
            ("Account", "golem_common::model::account::Account"),
            (
                "AccountCreation",
                "golem_common::model::account::AccountCreation",
            ),
            (
                "AccountSetPlan",
                "golem_common::model::account::AccountSetPlan",
            ),
            (
                "AccountSetRoles",
                "golem_common::model::account::AccountSetRoles",
            ),
            (
                "AccountUpdate",
                "golem_common::model::account::AccountUpdate",
            ),
            // agent
            (
                "JsonComponentModelValue",
                "golem_common::model::agent::JsonComponentModelValue",
            ),
            (
                "DeployedRegisteredAgentType",
                "golem_common::model::agent::DeployedRegisteredAgentType",
            ),
            (
                "RegisteredAgentTypeImplementer",
                "golem_common::model::agent::RegisteredAgentTypeImplementer",
            ),
            (
                "UntypedJsonDataValue",
                "golem_common::model::agent::UntypedJsonDataValue",
            ),
            (
                "UntypedDataValue",
                "golem_common::model::agent::UntypedDataValue",
            ),
            // application
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
            // auth
            ("Token", "golem_common::model::auth::Token"),
            ("TokenCreation", "golem_common::model::auth::TokenCreation"),
            (
                "TokenWithSecret",
                "golem_common::model::auth::TokenWithSecret",
            ),
            // component
            (
                "ComponentCreation",
                "golem_common::model::component::ComponentCreation",
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
                "ComponentUpdate",
                "golem_common::model::component::ComponentUpdate",
            ),
            // deployment
            ("Deployment", "golem_common::model::deployment::Deployment"),
            (
                "CurrentDeployment",
                "golem_common::model::deployment::CurrentDeployment",
            ),
            (
                "DeploymentCreation",
                "golem_common::model::deployment::DeploymentCreation",
            ),
            (
                "DeploymentRollback",
                "golem_common::model::deployment::DeploymentRollback",
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
            // domain_registration
            (
                "DomainRegistration",
                "golem_common::model::domain_registration::DomainRegistration",
            ),
            (
                "DomainRegistrationCreation",
                "golem_common::model::domain_registration::DomainRegistrationCreation",
            ),
            // environment
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
            (
                "EnvironmentWithDetails",
                "golem_common::model::environment::EnvironmentWithDetails",
            ),
            // environment_plugin_grant
            (
                "EnvironmentPluginGrant",
                "golem_common::model::environment_plugin_grant::EnvironmentPluginGrant",
            ),
            (
                "EnvironmentPluginGrantWithDetails",
                "golem_common::model::environment_plugin_grant::EnvironmentPluginGrantWithDetails",
            ),
            (
                "EnvironmentPluginGrantCreation",
                "golem_common::model::environment_plugin_grant::EnvironmentPluginGrantCreation",
            ),
            // environment_share
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
            // login
            (
                "OAuth2DeviceflowData",
                "golem_common::model::login::OAuth2DeviceflowData",
            ),
            (
                "OAuth2DeviceflowStart",
                "golem_common::model::login::OAuth2DeviceflowStart",
            ),
            (
                "OAuth2WebflowData",
                "golem_common::model::login::OAuth2WebflowData",
            ),
            // plan
            ("Plan", "golem_common::model::plan::Plan"),
            // plugin_registration
            (
                "ComponentTransformerPluginSpec",
                "golem_common::model::plugin_registration::ComponentTransformerPluginSpec",
            ),
            (
                "OplogProcessorPluginSpec",
                "golem_common::model::plugin_registration::OplogProcessorPluginSpec",
            ),
            (
                "PluginRegistrationCreation",
                "golem_common::model::plugin_registration::PluginRegistrationCreation",
            ),
            (
                "PluginRegistrationDto",
                "golem_common::model::plugin_registration::PluginRegistrationDto",
            ),
            // reports
            (
                "AccountCountsReport",
                "golem_common::model::reports::AccountCountsReport",
            ),
            (
                "AccountSummaryReport",
                "golem_common::model::reports::AccountSummaryReport",
            ),
            // security_scheme
            (
                "SecuritySchemeCreation",
                "golem_common::model::security_scheme::SecuritySchemeCreation",
            ),
            (
                "SecuritySchemeDto",
                "golem_common::model::security_scheme::SecuritySchemeDto",
            ),
            (
                "SecuritySchemeUpdate",
                "golem_common::model::security_scheme::SecuritySchemeUpdate",
            ),
            // worker
            (
                "FlatComponentFileSystemNode",
                "golem_common::model::worker::FlatComponentFileSystemNode",
            ),
            (
                "RevertWorkerTarget",
                "golem_common::model::worker::RevertWorkerTarget",
            ),
            (
                "WorkerCreationRequest",
                "golem_common::model::worker::WorkerCreationRequest",
            ),
            (
                "WorkerMetadataDto",
                "golem_common::model::worker::WorkerMetadataDto",
            ),
            (
                "WorkerUpdateMode",
                "golem_common::model::worker::WorkerUpdateMode",
            ),
            // oplog
            ("OplogCursor", "golem_common::model::oplog::OplogCursor"),
            (
                "PublicOplogEntry",
                "golem_common::model::oplog::PublicOplogEntry",
            ),
            (
                "PublicOplogEntryWithIndex",
                "golem_common::model::oplog::PublicOplogEntryWithIndex",
            ),
            // http_api_deployment
            (
                "HttpApiDeployment",
                "golem_common::model::http_api_deployment::HttpApiDeployment",
            ),
            (
                "HttpApiDeploymentCreation",
                "golem_common::model::http_api_deployment::HttpApiDeploymentCreation",
            ),
            (
                "HttpApiDeploymentUpdate",
                "golem_common::model::http_api_deployment::HttpApiDeploymentUpdate",
            ),
            // mcp_deployment
            (
                "McpDeployment",
                "golem_common::model::mcp_deployment::McpDeployment",
            ),
            (
                "McpDeploymentCreation",
                "golem_common::model::mcp_deployment::McpDeploymentCreation",
            ),
            (
                "McpDeploymentUpdate",
                "golem_common::model::mcp_deployment::McpDeploymentUpdate",
            ),
            // common
            ("Empty", "golem_common::model::Empty"),
            ("ErrorBody", "golem_common::model::error::ErrorBody"),
            ("ErrorsBody", "golem_common::model::error::ErrorsBody"),
            ("ScanCursor", "golem_common::model::ScanCursor"),
            ("UntypedJsonBody", "golem_common::model::UntypedJsonBody"),
            ("VersionInfo", "golem_common::model::VersionInfo"),
            ("WorkerFilter", "golem_common::model::WorkerFilter"),
            ("WorkerId", "golem_common::model::WorkerId"),
            (
                "WorkerResourceDescription",
                "golem_common::model::WorkerResourceDescription",
            ),
            ("WorkerStatus", "golem_common::model::WorkerStatus"),
            // golem_wasm
            ("AnalysedExport", "golem_wasm::analysis::AnalysedExport"),
            ("AnalysedType", "golem_wasm::analysis::AnalysedType"),
            (
                "ValueAndOptionalType",
                "golem_wasm::json::OptionallyValueAndTypeJson",
            ),
            ("ValueAndType", "golem_wasm::ValueAndType"),
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
