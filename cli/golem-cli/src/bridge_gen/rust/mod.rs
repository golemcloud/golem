// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Schema-native Rust bridge SDK generator.
//!
//! The generator walks the agent's schema-native [`AgentTypeSchema`] and emits
//! a Rust client crate whose request/response codecs build
//! [`golem_common::schema::SchemaValue`] trees directly and `serde_json`-encode
//! them onto the bare `serde_json::Value` request bodies (and decode the
//! `TypedSchemaValue` responses) — wire-identical to the TypeScript generator
//! and to the server's own (de)serialization, by construction. There is no
//! dependency on the legacy `AnalysedType` / `IntoValue` / `FromValue` surface.

use crate::bridge_gen::parameter_naming::ParameterNaming;
use crate::bridge_gen::rust::rust::{is_valid_rust_ident, to_rust_ident};
use crate::bridge_gen::type_naming::{TypeNaming, user_supplied_fields};
use crate::bridge_gen::{
    BridgeGenerator, BridgeMode, bridge_client_directory_name,
    bridge_client_directory_name_for_mode,
};
use crate::fs;
use crate::sdk_overrides::{sdk_overrides, workspace_root};
use anyhow::{anyhow, bail};
use camino::{Utf8Path, Utf8PathBuf};
use golem_common::model::agent::{AgentConfigSource, AgentMode};
use golem_common::schema::agent::{
    AgentConfigDeclarationSchema, AgentMethodSchema, AgentTypeSchema, InputSchema, OutputSchema,
    typed_schema_value_with_projected_defs,
};
use golem_common::schema::graph::SchemaTypeDef;
use golem_common::schema::multimodal::multimodal_variant_cases;
use golem_common::schema::schema_type::{
    BinaryRestrictions, SchemaType, TextRestrictions, VariantCaseType,
};
use golem_common::schema::schema_value::SchemaValue;
use golem_common::schema::unstructured::{
    unstructured_binary_restrictions, unstructured_text_restrictions,
};
use heck::{ToSnakeCase, ToUpperCamelCase};
use proc_macro2::{Ident, Span, TokenStream};
use quote::quote;
use syn::Index;
use toml_edit::{Array, DocumentMut, InlineTable, Item, Table, Value, value};
use tracing::debug;

#[allow(clippy::module_inception)]
mod rust;
pub mod tool;
mod type_name;

pub use type_name::RustTypeName;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RustBridgeMode {
    ExternalRest,
    GuestWasmRpc,
}

struct GuestMethodNames {
    await_name: Ident,
    trigger_name: Ident,
    schedule_name: Ident,
    internal_name: Ident,
    scheduled_time_param: Ident,
    param_names: Vec<String>,
}

#[derive(Clone, Copy, Debug)]
struct RustRuntimeConfig {
    mode: RustBridgeMode,
    reexport_golem_server_at_root: bool,
}

impl RustRuntimeConfig {
    fn new(mode: RustBridgeMode) -> Self {
        Self {
            mode,
            reexport_golem_server_at_root: false,
        }
    }

    fn with_root_golem_server_reexport(mut self, reexport: bool) -> Self {
        self.reexport_golem_server_at_root = reexport;
        self
    }

    fn generated_prelude(&self) -> TokenStream {
        match self.mode {
            RustBridgeMode::ExternalRest => {
                let root_golem_server_reexport = self
                    .reexport_golem_server_at_root
                    .then(|| quote! { pub use golem_client::bridge::GolemServer; });

                quote! {
                    #root_golem_server_reexport

                    pub mod __golem_bridge_runtime {
                    pub use golem_client::bridge::ClientError;
                    pub use golem_client::bridge::GolemServer;

                    pub mod schema {
                        pub use golem_common::schema::*;
                    }

                    pub mod agentic {
                        pub use golem_common::agentic::*;
                    }
                }
                }
            }
            RustBridgeMode::GuestWasmRpc => quote! {
                pub mod __golem_bridge_runtime {
                    #[derive(Debug, Clone)]
                    pub enum ClientError {
                        SchemaEncodeFailed { message: String },
                        SchemaDecodeFailed { message: String },
                        RpcFailed { message: String },
                        MissingResult { method: String },
                        ConfigEncodingFailed { message: String },
                    }

                    pub mod schema {
                        pub use golem_rust::SchemaValue;
                        pub use golem_rust::schema::{
                            DurationValuePayload, ResultValuePayload, UnionValuePayload,
                            VariantValuePayload,
                        };
                    }

                    pub mod agentic {
                        pub use golem_rust::agentic::*;
                    }
                }
            },
        }
    }
}

/// User-supplied input shape for a constructor or method.
enum RustInput {
    /// Ordinary positional parameters: `(param_name, schema)` in order.
    Params(Vec<(String, SchemaType)>),
    /// Multimodal input: `(case_name, payload_schema)` per modality.
    Multimodal(Vec<(String, SchemaType)>),
}

/// Output shape for a method.
#[allow(clippy::large_enum_variant)]
enum RustOutput {
    /// No return value.
    Unit,
    /// A single returned value of the given schema.
    Single(SchemaType),
    /// Multimodal output: `(case_name, payload_schema)` per modality.
    Multimodal(Vec<(String, SchemaType)>),
}

pub struct RustBridgeGenerator {
    target_path: Utf8PathBuf,
    agent_type: AgentTypeSchema,
    testing: bool,
    mode: RustBridgeMode,
    same_language: bool,

    type_naming: TypeNaming<RustTypeName>,
    /// Distinct text-language restriction sets discovered while generating, each
    /// mapped to the generated `crate::languages::*` enum name.
    generated_language_enums: Vec<(Vec<String>, String)>,
    /// Distinct binary mime-type restriction sets discovered while generating,
    /// each mapped to the generated `crate::mimetypes::*` enum name.
    generated_mimetypes_enums: Vec<(Vec<String>, String)>,
    /// Distinct multimodal modality sets discovered while generating, each
    /// mapped to the generated `crate::Multimodal*` enum name.
    known_multimodals: Vec<(Vec<(String, SchemaType)>, String)>,
}

impl BridgeGenerator for RustBridgeGenerator {
    fn new(
        agent_type: AgentTypeSchema,
        target_path: &Utf8Path,
        testing: bool,
    ) -> anyhow::Result<Self> {
        Self::new_with_mode(
            agent_type,
            target_path,
            testing,
            RustBridgeMode::ExternalRest,
        )
    }

    fn generate(&mut self) -> anyhow::Result<()> {
        let cargo_toml_path = self.target_path.join("Cargo.toml");
        let lib_rs_path = self.target_path.join("src/lib.rs");

        if !self.target_path.exists() {
            std::fs::create_dir_all(&self.target_path)?;
        }
        let src_dir = self.target_path.join("src");
        if !src_dir.exists() {
            std::fs::create_dir_all(&src_dir)?;
        }

        self.generate_cargo_toml(&cargo_toml_path)?;
        self.generate_lib_rs(&lib_rs_path)?;

        Ok(())
    }
}

impl RustBridgeGenerator {
    pub fn new_with_mode(
        agent_type: AgentTypeSchema,
        target_path: &Utf8Path,
        testing: bool,
        mode: RustBridgeMode,
    ) -> anyhow::Result<Self> {
        Self::new_with_mode_and_extra_reserved_names(
            agent_type,
            target_path,
            testing,
            mode,
            std::iter::empty::<String>(),
        )
    }

    pub(crate) fn new_guest_with_extra_reserved_names(
        agent_type: AgentTypeSchema,
        target_path: &Utf8Path,
        testing: bool,
        extra: impl IntoIterator<Item = String>,
    ) -> anyhow::Result<Self> {
        Self::new_with_mode_and_extra_reserved_names(
            agent_type,
            target_path,
            testing,
            RustBridgeMode::GuestWasmRpc,
            extra,
        )
    }

    fn new_with_mode_and_extra_reserved_names(
        agent_type: AgentTypeSchema,
        target_path: &Utf8Path,
        testing: bool,
        mode: RustBridgeMode,
        extra: impl IntoIterator<Item = String>,
    ) -> anyhow::Result<Self> {
        let same_language = agent_type.source_language.eq_ignore_ascii_case("rust");
        let type_naming = match mode {
            RustBridgeMode::ExternalRest => TypeNaming::new(&agent_type, same_language)?,
            RustBridgeMode::GuestWasmRpc => {
                let reserved_names = [
                    RustTypeName::Derived("__golem_bridge_runtime".to_string()),
                    RustTypeName::Derived(Self::guest_client_struct_name_string(
                        &agent_type.type_name.0,
                    )),
                    RustTypeName::Derived("languages".to_string()),
                    RustTypeName::Derived("mimetypes".to_string()),
                ]
                .into_iter()
                .chain(extra.into_iter().map(RustTypeName::Derived));
                TypeNaming::new_with_reserved_names(&agent_type, same_language, reserved_names)?
            }
        };

        Ok(Self {
            target_path: target_path.to_path_buf(),
            agent_type,
            testing,
            mode,
            same_language,
            type_naming,
            generated_language_enums: Vec::new(),
            generated_mimetypes_enums: Vec::new(),
            known_multimodals: Vec::new(),
        })
    }

    /// Generates the Cargo.toml manifest file
    fn generate_cargo_toml(&self, path: &Utf8Path) -> anyhow::Result<()> {
        let golem_source = if self.testing {
            // In test mode, use the local workspace path, so we always test against the current code
            GolemDependencySource::Path(workspace_root()?)
        } else {
            match sdk_overrides()?.golem_repo_root()? {
                Some(repo_root) => GolemDependencySource::Path(repo_root),
                None => GolemDependencySource::GitMain,
            }
        };

        let mut doc = DocumentMut::new();

        doc["package"] = Item::Table(Default::default());
        doc["package"]["name"] = value(self.package_crate_name());
        doc["package"]["version"] = value("0.0.1");
        doc["package"]["edition"] = value("2021");
        doc["package"]["description"] = value("Generated by golem-cli");

        doc["dependencies"] = Item::Table(Table::default());
        doc["dependencies"]["chrono"] = dep("0.4", &[]);
        match self.mode {
            RustBridgeMode::ExternalRest => {
                doc["dependencies"]["golem-client"] = golem_source.dep_item("golem-client", &[])?;
                doc["dependencies"]["golem-common"] =
                    golem_source.dep_item("golem-common", &["client"])?;
                doc["dependencies"]["reqwest"] = dep("0.13", &["rustls"]);
                doc["dependencies"]["reqwest-middleware"] = dep("0.5", &[]);
            }
            RustBridgeMode::GuestWasmRpc => {
                doc["dependencies"]["golem-rust"] = golem_source
                    .dep_item("sdks/rust/golem-rust", &["export_golem_agentic", "macro"])?;
            }
        }
        doc["dependencies"]["serde"] = dep("1", &["derive"]);
        doc["dependencies"]["serde_json"] = dep("1", &[]);
        doc["dependencies"]["uuid"] = dep("1.18.1", &["v4"]);

        std::fs::write(path, doc.to_string())
            .map_err(|e| anyhow!("Failed to write Cargo.toml file: {e}"))?;

        Ok(())
    }

    /// Generates the lib.rs source file
    fn generate_lib_rs(&mut self, path: &Utf8Path) -> anyhow::Result<()> {
        let tokens = self.generate_lib_rs_tokens()?;
        debug!("raw lib.rs:\n {}", tokens);
        let formatted = prettyplease::unparse(&syn::parse2(tokens)?);
        std::fs::write(path, formatted).map_err(|e| anyhow!("Failed to write lib.rs file: {e}"))?;
        Ok(())
    }

    /// Generates the TokenStream for lib.rs content
    fn generate_lib_rs_tokens(&mut self) -> anyhow::Result<TokenStream> {
        match self.mode {
            RustBridgeMode::ExternalRest => self.generate_external_lib_rs_tokens(),
            RustBridgeMode::GuestWasmRpc => self.generate_guest_lib_rs_tokens(),
        }
    }

    fn generate_external_lib_rs_tokens(&mut self) -> anyhow::Result<TokenStream> {
        let agent_type_name = self.agent_type.type_name.0.clone();
        let client_struct_name = Ident::new(&agent_type_name, Span::call_site());

        let constructor_input = self.agent_type.constructor.input_schema.clone();
        let constructor_param_defs = self.input_param_defs(&constructor_input)?;
        let constructor_params_value = self.input_param_value(&constructor_input)?;

        let mut methods = Vec::new();
        for method in self.agent_type.methods.clone() {
            methods.extend(self.methods(&method)?);
        }

        let local_configs: Vec<AgentConfigDeclarationSchema> = self
            .agent_type
            .config
            .iter()
            .filter(|c| c.source == AgentConfigSource::Local)
            .cloned()
            .collect();

        let mut config_param_defs = Vec::new();
        let mut config_encode_stmts = Vec::new();
        for config in &local_configs {
            let param_name_str = format!(
                "config_{}",
                config
                    .path
                    .iter()
                    .map(|s| s.to_snake_case())
                    .collect::<Vec<_>>()
                    .join("_")
            );
            let param_name = Ident::new(&self.to_rust_ident(&param_name_str), Span::call_site());
            let param_type = self.type_reference(&config.value_type, false)?;
            config_param_defs.push(quote! { #param_name: Option<#param_type> });

            let path_segments: Vec<TokenStream> = config
                .path
                .iter()
                .map(|s| quote! { #s.to_string() })
                .collect();
            let value_encode =
                self.emit_encode_expr(quote! { value }, &config.value_type, false, 0)?;
            config_encode_stmts.push(quote! {
                if let Some(value) = #param_name {
                    let __config_value: crate::__golem_bridge_runtime::schema::SchemaValue = (|| -> Result<crate::__golem_bridge_runtime::schema::SchemaValue, String> {
                        #value_encode
                    })().map_err(|__e| crate::__golem_bridge_runtime::ClientError::InvocationFailed { message: format!("Failed to encode config value: {__e}") })?;
                    let __config_json = serde_json::to_value(&__config_value).map_err(|__e| crate::__golem_bridge_runtime::ClientError::InvocationFailed { message: format!("Failed to serialize config value: {__e}") })?;
                    agent_config.push(golem_client::model::AgentConfigEntryDto {
                        path: vec![#(#path_segments),*],
                        value: __config_json.into(),
                    });
                }
            });
        }

        let get_with_config_method = if self.agent_type.mode == AgentMode::Durable {
            quote! {
                pub async fn get_with_config(#(#constructor_param_defs,)* #(#config_param_defs,)*) -> Result<Self, crate::__golem_bridge_runtime::ClientError> {
                    let constructor_parameters: serde_json::Value = #constructor_params_value;
                    let mut agent_config = Vec::new();
                    #(#config_encode_stmts)*
                    Self::__create(constructor_parameters, None, agent_config).await
                }
            }
        } else {
            quote! {}
        };

        let with_config_methods = if !local_configs.is_empty() {
            quote! {
                #get_with_config_method

                pub async fn get_phantom_with_config(uuid: uuid::Uuid, #(#constructor_param_defs,)* #(#config_param_defs,)*) -> Result<Self, crate::__golem_bridge_runtime::ClientError> {
                    let constructor_parameters: serde_json::Value = #constructor_params_value;
                    let mut agent_config = Vec::new();
                    #(#config_encode_stmts)*
                    Self::__create(constructor_parameters, Some(uuid), agent_config).await
                }

                pub async fn new_phantom_with_config(#(#constructor_param_defs,)* #(#config_param_defs,)*) -> Result<Self, crate::__golem_bridge_runtime::ClientError> {
                    let constructor_parameters: serde_json::Value = #constructor_params_value;
                    let mut agent_config = Vec::new();
                    #(#config_encode_stmts)*
                    Self::__create(constructor_parameters, Some(uuid::Uuid::new_v4()), agent_config).await
                }
            }
        } else {
            quote! {}
        };

        let global_config = self.global_config();

        let get_method = if self.agent_type.mode == AgentMode::Durable {
            quote! {
                pub async fn get(#(#constructor_param_defs),*) -> Result<Self, crate::__golem_bridge_runtime::ClientError> {
                    let constructor_parameters: serde_json::Value = #constructor_params_value;
                    Self::__create(constructor_parameters, None, vec![]).await
                }
            }
        } else {
            quote! {}
        };

        // Type definitions + codecs are generated last so all language /
        // mimetype / multimodal enums discovered while emitting methods are
        // already registered.
        let type_definitions = self.type_definitions()?;
        let multimodals = self.multimodals()?;
        let languages = self.languages_module();
        let mimetypes = self.mimetypes_module();
        let runtime_prelude = RustRuntimeConfig::new(self.mode)
            .with_root_golem_server_reexport(!self.root_type_name_conflicts("GolemServer"))
            .generated_prelude();

        let tokens = quote! {
            #![allow(unused)]
            #![allow(non_snake_case)]
            #![allow(clippy::all)]

            #runtime_prelude

            #[derive(Debug, Clone)]
            pub struct #client_struct_name {
                constructor_parameters: serde_json::Value,
                phantom_id: Option<uuid::Uuid>,
                agent_id: golem_client::model::AgentId,
            }

            impl #client_struct_name {
                #get_method

                pub async fn get_phantom(uuid: uuid::Uuid, #(#constructor_param_defs),*) -> Result<Self, crate::__golem_bridge_runtime::ClientError> {
                    let constructor_parameters: serde_json::Value = #constructor_params_value;
                    Self::__create(constructor_parameters, Some(uuid), vec![]).await
                }

                pub async fn new_phantom(#(#constructor_param_defs),*) -> Result<Self, crate::__golem_bridge_runtime::ClientError> {
                    let constructor_parameters: serde_json::Value = #constructor_params_value;
                    Self::__create(constructor_parameters, Some(uuid::Uuid::new_v4()), vec![]).await
                }

                #with_config_methods

                /// Returns the agent's identity, containing the component ID and agent name.
                pub fn agent_id(&self) -> &golem_client::model::AgentId {
                    &self.agent_id
                }

                /// Returns the configured worker service URL.
                pub fn worker_service_url() -> reqwest::Url {
                    CONFIG.get().expect("Configuration has not been set").server.url()
                }

                /// Returns the configured authentication token.
                pub fn auth_token() -> golem_client::Security {
                    CONFIG.get().expect("Configuration has not been set").server.token()
                }

                async fn __create(
                    constructor_parameters: serde_json::Value,
                    phantom_id: Option<uuid::Uuid>,
                    agent_config: Vec<golem_client::model::AgentConfigEntryDto>,
                ) -> Result<Self, crate::__golem_bridge_runtime::ClientError> {
                    let config = CONFIG.get().expect("Configuration has not been set");

                    let client = reqwest_middleware::ClientWithMiddleware::from(
                        reqwest::Client::builder().build().unwrap()
                    );
                    let context = golem_client::Context {
                        client,
                        base_url: config.server.url(),
                        security_token: config.server.token(),
                    };
                    let api_client = golem_client::api::AgentClientLive { context };
                    let response = golem_client::api::AgentClient::create_agent(
                        &api_client,
                        &golem_client::model::CreateAgentRequest {
                            app_name: config.app_name.to_string(),
                            env_name: config.env_name.to_string(),
                            agent_type_name: #agent_type_name.to_string(),
                            parameters: constructor_parameters.clone(),
                            phantom_id,
                            config: Some(agent_config),
                        },
                    ).await?;

                    Ok(Self { constructor_parameters, phantom_id, agent_id: response.agent_id })
                }

                #(#methods)*

                async fn invoke(
                    &self,
                    method_name: &str,
                    method_parameters: serde_json::Value,
                    mode: golem_client::model::AgentInvocationMode,
                    schedule_at: Option<chrono::DateTime<chrono::Utc>>,
                ) -> Result<Option<golem_client::model::TypedSchemaValue>, crate::__golem_bridge_runtime::ClientError> {
                    let config = CONFIG.get().expect("Configuration has not been set");

                    let client = reqwest_middleware::ClientWithMiddleware::from(
                        reqwest::Client::builder().build().unwrap()
                    );
                    let context = golem_client::Context {
                        client,
                        base_url: config.server.url(),
                        security_token: config.server.token(),
                    };
                    let client = golem_client::api::AgentClientLive { context };
                    let response = golem_client::api::AgentClient::invoke_agent(
                        &client,
                        None,
                        &golem_client::model::AgentInvocationRequest {
                            app_name: config.app_name.to_string(),
                            env_name: config.env_name.to_string(),
                            agent_type_name: #agent_type_name.to_string(),
                            parameters: self.constructor_parameters.clone(),
                            phantom_id: self.phantom_id,
                            method_name: method_name.to_string(),
                            method_parameters,
                            mode,
                            schedule_at,
                            idempotency_key: None,
                            deployment_revision: None,
                            owner_account_email: None,
                        },
                    )
                    .await?;
                    Ok(response.result)
                }
            }

            #global_config

            #type_definitions

            #multimodals

            #languages

            #mimetypes
        };

        Ok(tokens)
    }

    fn generate_guest_lib_rs_tokens(&mut self) -> anyhow::Result<TokenStream> {
        let agent_type_name = self.agent_type.type_name.0.clone();
        let client_struct_name = Ident::new(
            &Self::guest_client_struct_name_string(&agent_type_name),
            Span::call_site(),
        );

        let constructor_input = self.agent_type.constructor.input_schema.clone();
        let constructor_param_names = self.input_param_ident_names_unique(&constructor_input)?;
        let constructor_param_defs =
            self.input_param_defs_with_ident_names(&constructor_input, &constructor_param_names)?;
        let constructor_param_refs =
            self.input_param_refs_with_ident_names(&constructor_param_names);
        let constructor_params_value = self.input_param_schema_value_with_ident_names(
            &constructor_input,
            &constructor_param_names,
        )?;

        let local_configs: Vec<AgentConfigDeclarationSchema> = self
            .agent_type
            .config
            .iter()
            .filter(|c| c.source == AgentConfigSource::Local)
            .cloned()
            .collect();

        let guest_method_names = self.allocate_guest_method_names(!local_configs.is_empty())?;

        let mut methods = Vec::new();
        for (method, names) in self
            .agent_type
            .methods
            .clone()
            .iter()
            .zip(guest_method_names.iter())
        {
            methods.extend(self.guest_methods(method, names)?);
        }

        let mut create_signature_names = ParameterNaming::new();
        create_signature_names.reserve_many(constructor_param_names.clone());
        let phantom_id_param = Self::ident_from_name(
            create_signature_names.fresh(self.to_rust_ident("__golem_bridge_phantom_id")),
        );
        let agent_config_param = Self::ident_from_name(
            create_signature_names.fresh(self.to_rust_ident("__golem_bridge_agent_config")),
        );

        let mut config_param_defs = Vec::new();
        let mut config_encode_stmts = Vec::new();
        let mut config_names = ParameterNaming::new();
        config_names.reserve_many(constructor_param_names.clone());
        config_names.reserve(phantom_id_param.to_string());
        let agent_config_values = Self::ident_from_name(
            config_names.fresh(self.to_rust_ident("__golem_bridge_agent_config_values")),
        );
        for config in &local_configs {
            let param_name_str = format!(
                "__golem_bridge_config_{}",
                config
                    .path
                    .iter()
                    .map(|s| s.to_snake_case())
                    .collect::<Vec<_>>()
                    .join("_")
            );
            let param_name =
                Self::ident_from_name(config_names.fresh(self.to_rust_ident(&param_name_str)));
            let param_type = self.type_reference(&config.value_type, false)?;
            config_param_defs.push(quote! { #param_name: Option<#param_type> });

            let path_segments: Vec<TokenStream> = config
                .path
                .iter()
                .map(|s| quote! { #s.to_string() })
                .collect();
            let value_encode =
                self.emit_encode_expr(quote! { value }, &config.value_type, false, 0)?;
            let config_graph = typed_schema_value_with_projected_defs(
                &self.agent_type.schema,
                config.value_type.clone(),
                SchemaValue::Bool(false),
            )
            .graph()
            .clone();
            let schema_graph_json = serde_json::to_string(&config_graph)?;
            config_encode_stmts.push(quote! {
                if let Some(value) = #param_name {
                    let __config_value: crate::__golem_bridge_runtime::schema::SchemaValue = (|| -> Result<crate::__golem_bridge_runtime::schema::SchemaValue, String> {
                        #value_encode
                    })().map_err(|__e| crate::__golem_bridge_runtime::ClientError::ConfigEncodingFailed { message: __e })?;
                    let __config_graph: golem_rust::SchemaGraph = serde_json::from_str(#schema_graph_json)
                        .map_err(|__e| crate::__golem_bridge_runtime::ClientError::ConfigEncodingFailed { message: format!("Failed to deserialize config schema: {__e}") })?;
                    let __typed = golem_rust::TypedSchemaValue::new(__config_graph, __config_value);
                    #agent_config_values.push(golem_rust::golem_agentic::golem::agent::common::TypedAgentConfigValue {
                        path: vec![#(#path_segments),*],
                        value: golem_rust::encode_typed_schema_value(&__typed)
                            .map_err(|__e| crate::__golem_bridge_runtime::ClientError::ConfigEncodingFailed { message: format!("Failed to encode config value: {__e}") })?,
                    });
                }
            });
        }

        let get_with_config_method = if self.agent_type.mode == AgentMode::Durable {
            quote! {
                pub fn get_with_config(#(#constructor_param_defs,)* #(#config_param_defs,)*) -> Result<Self, crate::__golem_bridge_runtime::ClientError> {
                    let mut #agent_config_values = Vec::new();
                    #(#config_encode_stmts)*
                    Self::__create(None, #agent_config_values, #(#constructor_param_refs),*)
                }
            }
        } else {
            quote! {}
        };

        let with_config_methods = if !local_configs.is_empty() {
            quote! {
                #get_with_config_method

                pub fn get_phantom_with_config(#phantom_id_param: golem_rust::Uuid, #(#constructor_param_defs,)* #(#config_param_defs,)*) -> Result<Self, crate::__golem_bridge_runtime::ClientError> {
                    let mut #agent_config_values = Vec::new();
                    #(#config_encode_stmts)*
                    Self::__create(Some(#phantom_id_param), #agent_config_values, #(#constructor_param_refs),*)
                }

                pub fn new_phantom_with_config(#(#constructor_param_defs,)* #(#config_param_defs,)*) -> Result<Self, crate::__golem_bridge_runtime::ClientError> {
                    let mut #agent_config_values = Vec::new();
                    #(#config_encode_stmts)*
                    Self::__create(Some(golem_rust::Uuid::new_v4()), #agent_config_values, #(#constructor_param_refs),*)
                }
            }
        } else {
            quote! {}
        };

        let get_method = if self.agent_type.mode == AgentMode::Durable {
            quote! {
                pub fn get(#(#constructor_param_defs),*) -> Result<Self, crate::__golem_bridge_runtime::ClientError> {
                    Self::__create(None, Vec::new(), #(#constructor_param_refs),*)
                }
            }
        } else {
            quote! {}
        };

        let type_definitions = self.type_definitions()?;
        let multimodals = self.multimodals()?;
        let languages = self.languages_module();
        let mimetypes = self.mimetypes_module();
        let runtime_prelude = RustRuntimeConfig::new(self.mode).generated_prelude();

        let tokens = quote! {
            #![allow(unused)]
            #![allow(non_snake_case)]
            #![allow(clippy::all)]

            #runtime_prelude

            #[derive(Debug)]
            pub struct #client_struct_name {
                agent_id: String,
                phantom_id: Option<golem_rust::Uuid>,
                wasm_rpc: golem_rust::golem_agentic::golem::agent::host::WasmRpc,
            }

            impl #client_struct_name {
                #get_method

                pub fn get_phantom(#phantom_id_param: golem_rust::Uuid, #(#constructor_param_defs),*) -> Result<Self, crate::__golem_bridge_runtime::ClientError> {
                    Self::__create(Some(#phantom_id_param), Vec::new(), #(#constructor_param_refs),*)
                }

                pub fn new_phantom(#(#constructor_param_defs),*) -> Result<Self, crate::__golem_bridge_runtime::ClientError> {
                    Self::__create(Some(golem_rust::Uuid::new_v4()), Vec::new(), #(#constructor_param_refs),*)
                }

                #with_config_methods

                fn __create(
                    #phantom_id_param: Option<golem_rust::Uuid>,
                    #agent_config_param: Vec<golem_rust::golem_agentic::golem::agent::common::TypedAgentConfigValue>,
                    #(#constructor_param_defs),*
                ) -> Result<Self, crate::__golem_bridge_runtime::ClientError> {
                    let constructor_value: crate::__golem_bridge_runtime::schema::SchemaValue = #constructor_params_value;
                    let agent_id = golem_rust::golem_agentic::golem::agent::host::make_agent_id(
                        #agent_type_name,
                        golem_rust::encode_schema_value(&constructor_value)
                            .map_err(|__e| crate::__golem_bridge_runtime::ClientError::SchemaEncodeFailed { message: __e.to_string() })?,
                        #phantom_id_param.map(Into::into),
                    ).map_err(|__e| crate::__golem_bridge_runtime::ClientError::RpcFailed { message: format!("{__e:?}") })?;

                    let wasm_rpc = golem_rust::golem_agentic::golem::agent::host::WasmRpc::new(
                        #agent_type_name,
                        golem_rust::encode_schema_value(&constructor_value)
                            .map_err(|__e| crate::__golem_bridge_runtime::ClientError::SchemaEncodeFailed { message: __e.to_string() })?,
                        #phantom_id_param.map(Into::into),
                        #agent_config_param,
                    );

                    Ok(Self { agent_id, phantom_id: #phantom_id_param, wasm_rpc })
                }

                #(#methods)*
            }

            #type_definitions

            #multimodals

            #languages

            #mimetypes
        };

        Ok(tokens)
    }

    fn global_config(&self) -> TokenStream {
        quote! {
            static CONFIG: std::sync::OnceLock<golem_client::bridge::Configuration> = std::sync::OnceLock::new();

            pub fn configure(server: golem_client::bridge::GolemServer, app_name: &str, env_name: &str) {
                CONFIG
                    .set(golem_client::bridge::Configuration {
                        app_name: app_name.to_string(),
                        env_name: env_name.to_string(),
                        server,
                    })
                    .map_err(|_| ())
                    .expect("Configuration has already been set");
            }
        }
    }

    // --- Input / output shape resolution -----------------------------------

    fn rust_input(&self, input: &InputSchema) -> anyhow::Result<RustInput> {
        let fields = user_supplied_fields(input);
        if let [field] = fields.as_slice()
            && let Some(cases) = multimodal_variant_cases(self.type_naming.graph(), &field.schema)?
                .map(|c| c.to_vec())
        {
            return Ok(RustInput::Multimodal(self.multimodal_pairs(&cases)?));
        }
        Ok(RustInput::Params(
            fields
                .iter()
                .map(|f| (f.name.clone(), f.schema.clone()))
                .collect(),
        ))
    }

    fn rust_output(&self, output: &OutputSchema) -> anyhow::Result<RustOutput> {
        match output {
            OutputSchema::Unit => Ok(RustOutput::Unit),
            OutputSchema::Single(ty) => {
                if let Some(cases) =
                    multimodal_variant_cases(self.type_naming.graph(), ty)?.map(|c| c.to_vec())
                {
                    return Ok(RustOutput::Multimodal(self.multimodal_pairs(&cases)?));
                }
                Ok(RustOutput::Single((**ty).clone()))
            }
        }
    }

    fn multimodal_pairs(
        &self,
        cases: &[VariantCaseType],
    ) -> anyhow::Result<Vec<(String, SchemaType)>> {
        cases
            .iter()
            .map(|case| {
                let payload = case.payload.clone().ok_or_else(|| {
                    anyhow!(
                        "Multimodal case `{}` has no payload schema; expected a modality body",
                        case.name
                    )
                })?;
                Ok((case.name.clone(), payload))
            })
            .collect()
    }

    /// Resolve a [`SchemaType::Ref`] against [`TypeNaming::graph`] and return an
    /// owned copy of the def body (so callers can recurse with `&mut self`).
    fn resolve_ref_owned(&self, typ: &SchemaType) -> SchemaType {
        match typ {
            SchemaType::Ref { id, .. } => {
                let def: &SchemaTypeDef = self
                    .type_naming
                    .graph()
                    .lookup(id)
                    .expect("Ref points to a def in the shared graph");
                def.body.clone()
            }
            other => other.clone(),
        }
    }

    // --- Method generation --------------------------------------------------

    fn methods(&mut self, method: &AgentMethodSchema) -> anyhow::Result<Vec<TokenStream>> {
        Ok(vec![
            self.await_method(method)?,
            self.trigger_method(method)?,
            self.schedule_method(method)?,
            self.internal_method(method)?,
        ])
    }

    fn guest_methods(
        &mut self,
        method: &AgentMethodSchema,
        names: &GuestMethodNames,
    ) -> anyhow::Result<Vec<TokenStream>> {
        Ok(vec![
            self.guest_await_method(method, names)?,
            self.guest_trigger_method(method, names)?,
            self.guest_schedule_method(method, names)?,
            self.guest_internal_method(method, names)?,
        ])
    }

    fn allocate_guest_method_names(
        &self,
        has_local_configs: bool,
    ) -> anyhow::Result<Vec<GuestMethodNames>> {
        let mut impl_names = ParameterNaming::new();
        impl_names.reserve("get_phantom");
        impl_names.reserve("new_phantom");
        impl_names.reserve("__create");
        if self.agent_type.mode == AgentMode::Durable {
            impl_names.reserve("get");
        }
        if has_local_configs {
            impl_names.reserve("get_with_config");
            impl_names.reserve("get_phantom_with_config");
            impl_names.reserve("new_phantom_with_config");
        }

        let user_bases = self
            .agent_type
            .methods
            .iter()
            .map(|method| self.to_rust_ident(&method.name))
            .collect::<Vec<_>>();

        let await_names = user_bases
            .iter()
            .map(|base| Self::ident_from_name(impl_names.fresh(base)))
            .collect::<Vec<_>>();
        let trigger_names = user_bases
            .iter()
            .map(|base| {
                Self::ident_from_name(impl_names.fresh(format!("__golem_bridge_trigger_{base}")))
            })
            .collect::<Vec<_>>();
        let schedule_names = user_bases
            .iter()
            .map(|base| {
                Self::ident_from_name(impl_names.fresh(format!("__golem_bridge_schedule_{base}")))
            })
            .collect::<Vec<_>>();
        let internal_names = user_bases
            .iter()
            .map(|base| {
                Self::ident_from_name(impl_names.fresh(format!("__golem_bridge_invoke_{base}")))
            })
            .collect::<Vec<_>>();

        self.agent_type
            .methods
            .iter()
            .zip(await_names)
            .zip(trigger_names)
            .zip(schedule_names)
            .zip(internal_names)
            .map(
                |((((method, await_name), trigger_name), schedule_name), internal_name)| {
                    let param_names = self.input_param_ident_names_unique(&method.input_schema)?;
                    let mut signature_names = ParameterNaming::new();
                    signature_names.reserve_many(param_names.clone());
                    let scheduled_time_param = Self::ident_from_name(
                        signature_names.fresh(self.to_rust_ident("__golem_bridge_scheduled_time")),
                    );
                    Ok(GuestMethodNames {
                        await_name,
                        trigger_name,
                        schedule_name,
                        internal_name,
                        scheduled_time_param,
                        param_names,
                    })
                },
            )
            .collect()
    }

    fn method_ident(&self, method: &AgentMethodSchema) -> Ident {
        let name = self.to_rust_ident(&method.name);
        match self.mode {
            RustBridgeMode::ExternalRest => Ident::new(&name, Span::call_site()),
            RustBridgeMode::GuestWasmRpc => {
                self.unique_internal_ident(&name, &self.guest_reserved_method_names())
            }
        }
    }

    fn trigger_method_ident(&self, method: &AgentMethodSchema) -> Ident {
        let name = match self.mode {
            RustBridgeMode::ExternalRest => format!("trigger_{}", self.to_rust_ident(&method.name)),
            RustBridgeMode::GuestWasmRpc => {
                format!(
                    "__golem_bridge_trigger_{}",
                    self.to_rust_ident(&method.name)
                )
            }
        };
        match self.mode {
            RustBridgeMode::ExternalRest => Ident::new(&name, Span::call_site()),
            RustBridgeMode::GuestWasmRpc => {
                self.unique_internal_ident(&name, &self.guest_user_method_ident_names())
            }
        }
    }

    fn schedule_method_ident(&self, method: &AgentMethodSchema) -> Ident {
        let name = match self.mode {
            RustBridgeMode::ExternalRest => {
                format!("schedule_{}", self.to_rust_ident(&method.name))
            }
            RustBridgeMode::GuestWasmRpc => {
                format!(
                    "__golem_bridge_schedule_{}",
                    self.to_rust_ident(&method.name)
                )
            }
        };
        match self.mode {
            RustBridgeMode::ExternalRest => Ident::new(&name, Span::call_site()),
            RustBridgeMode::GuestWasmRpc => {
                self.unique_internal_ident(&name, &self.guest_user_method_ident_names())
            }
        }
    }

    fn internal_method_ident(&self, method: &AgentMethodSchema) -> Ident {
        let name = match self.mode {
            RustBridgeMode::ExternalRest => format!("__{}", self.to_rust_ident(&method.name)),
            RustBridgeMode::GuestWasmRpc => {
                format!("__golem_bridge_invoke_{}", self.to_rust_ident(&method.name))
            }
        };
        match self.mode {
            RustBridgeMode::ExternalRest => Ident::new(&name, Span::call_site()),
            RustBridgeMode::GuestWasmRpc => {
                self.unique_internal_ident(&name, &self.guest_user_method_ident_names())
            }
        }
    }

    fn guest_user_method_ident_names(&self) -> Vec<String> {
        self.agent_type
            .methods
            .iter()
            .map(|method| self.to_rust_ident(&method.name))
            .collect()
    }

    fn guest_reserved_method_names(&self) -> Vec<String> {
        vec![
            "get".to_string(),
            "get_phantom".to_string(),
            "new_phantom".to_string(),
            "get_with_config".to_string(),
            "get_phantom_with_config".to_string(),
            "new_phantom_with_config".to_string(),
        ]
    }

    fn guest_await_method(
        &mut self,
        method: &AgentMethodSchema,
        names: &GuestMethodNames,
    ) -> anyhow::Result<TokenStream> {
        let name = &names.await_name;
        let internal_name = &names.internal_name;
        let return_type = self.output_return_type(&method.output_schema)?;
        let param_defs =
            self.input_param_defs_with_ident_names(&method.input_schema, &names.param_names)?;
        let param_refs = self.input_param_refs_with_ident_names(&names.param_names);
        let name_lit = method.name.as_str();

        match return_type {
            Some(return_type) => Ok(quote! {
                pub async fn #name(&self, #(#param_defs),*) -> Result<#return_type, crate::__golem_bridge_runtime::ClientError> {
                    let result = self.#internal_name(#(#param_refs),*).await?;
                    result.ok_or_else(|| crate::__golem_bridge_runtime::ClientError::MissingResult { method: #name_lit.to_string() })
                }
            }),
            None => Ok(quote! {
                pub async fn #name(&self, #(#param_defs),*) -> Result<(), crate::__golem_bridge_runtime::ClientError> {
                    let _result = self.#internal_name(#(#param_refs),*).await?;
                    Ok(())
                }
            }),
        }
    }

    fn guest_trigger_method(
        &mut self,
        method: &AgentMethodSchema,
        names: &GuestMethodNames,
    ) -> anyhow::Result<TokenStream> {
        let name = &names.trigger_name;
        let param_defs =
            self.input_param_defs_with_ident_names(&method.input_schema, &names.param_names)?;
        let name_lit = method.name.as_str();
        let params_schema_value = self
            .input_param_schema_value_with_ident_names(&method.input_schema, &names.param_names)?;

        Ok(quote! {
            pub fn #name(&self, #(#param_defs),*) -> Result<(), crate::__golem_bridge_runtime::ClientError> {
                let method_parameters: crate::__golem_bridge_runtime::schema::SchemaValue = #params_schema_value;
                let method_parameters = golem_rust::encode_schema_value(&method_parameters)
                    .map_err(|__e| crate::__golem_bridge_runtime::ClientError::SchemaEncodeFailed { message: __e.to_string() })?;
                self.wasm_rpc.invoke(#name_lit, method_parameters)
                    .map_err(|__e| crate::__golem_bridge_runtime::ClientError::RpcFailed { message: format!("{__e:?}") })
            }
        })
    }

    fn guest_schedule_method(
        &mut self,
        method: &AgentMethodSchema,
        names: &GuestMethodNames,
    ) -> anyhow::Result<TokenStream> {
        let name = &names.schedule_name;
        let scheduled_time_param = &names.scheduled_time_param;
        let param_defs =
            self.input_param_defs_with_ident_names(&method.input_schema, &names.param_names)?;
        let name_lit = method.name.as_str();
        let params_schema_value = self
            .input_param_schema_value_with_ident_names(&method.input_schema, &names.param_names)?;

        Ok(quote! {
            pub fn #name(&self, #scheduled_time_param: golem_rust::wasip2::clocks::wall_clock::Datetime, #(#param_defs),*) -> Result<(), crate::__golem_bridge_runtime::ClientError> {
                let method_parameters: crate::__golem_bridge_runtime::schema::SchemaValue = #params_schema_value;
                let method_parameters = golem_rust::encode_schema_value(&method_parameters)
                    .map_err(|__e| crate::__golem_bridge_runtime::ClientError::SchemaEncodeFailed { message: __e.to_string() })?;
                self.wasm_rpc.schedule_invocation(#scheduled_time_param, #name_lit, method_parameters);
                Ok(())
            }
        })
    }

    fn guest_internal_method(
        &mut self,
        method: &AgentMethodSchema,
        names: &GuestMethodNames,
    ) -> anyhow::Result<TokenStream> {
        let name_lit = method.name.as_str();
        let name = &names.internal_name;
        let param_defs =
            self.input_param_defs_with_ident_names(&method.input_schema, &names.param_names)?;
        let return_type = self.output_return_type(&method.output_schema)?;
        let params_schema_value = self
            .input_param_schema_value_with_ident_names(&method.input_schema, &names.param_names)?;

        match return_type {
            Some(return_type) => {
                let decode_body = self.output_decode_expr(&method.output_schema)?;
                Ok(quote! {
                    async fn #name(&self, #(#param_defs),*) -> Result<Option<#return_type>, crate::__golem_bridge_runtime::ClientError> {
                        let method_parameters: crate::__golem_bridge_runtime::schema::SchemaValue = #params_schema_value;
                        let method_parameters = golem_rust::encode_schema_value(&method_parameters)
                            .map_err(|__e| crate::__golem_bridge_runtime::ClientError::SchemaEncodeFailed { message: __e.to_string() })?;
                        let rpc_result_future = self.wasm_rpc.async_invoke_and_await(#name_lit, method_parameters);
                        let response = golem_rust::agentic::await_invoke_schema_value_result(rpc_result_future).await
                            .map_err(|__e| crate::__golem_bridge_runtime::ClientError::RpcFailed { message: format!("{__e:?}") })?;
                        match response {
                            Some(__value) => {
                                let __decoded: #return_type = (|| -> Result<#return_type, String> {
                                    #decode_body
                                })().map_err(|__e| crate::__golem_bridge_runtime::ClientError::SchemaDecodeFailed { message: __e })?;
                                Ok(Some(__decoded))
                            }
                            None => Ok(None),
                        }
                    }
                })
            }
            None => Ok(quote! {
                async fn #name(&self, #(#param_defs),*) -> Result<Option<()>, crate::__golem_bridge_runtime::ClientError> {
                    let method_parameters: crate::__golem_bridge_runtime::schema::SchemaValue = #params_schema_value;
                    let method_parameters = golem_rust::encode_schema_value(&method_parameters)
                        .map_err(|__e| crate::__golem_bridge_runtime::ClientError::SchemaEncodeFailed { message: __e.to_string() })?;
                    let rpc_result_future = self.wasm_rpc.async_invoke_and_await(#name_lit, method_parameters);
                    let _response = golem_rust::agentic::await_invoke_schema_value_result(rpc_result_future).await
                        .map_err(|__e| crate::__golem_bridge_runtime::ClientError::RpcFailed { message: format!("{__e:?}") })?;
                    Ok(Some(()))
                }
            }),
        }
    }

    fn await_method(&mut self, method: &AgentMethodSchema) -> anyhow::Result<TokenStream> {
        let name = self.method_ident(method);
        let internal_name = self.internal_method_ident(method);
        let return_type = self.output_return_type(&method.output_schema)?;
        let param_defs = self.input_param_defs(&method.input_schema)?;
        let param_refs = self.input_param_refs(&method.input_schema)?;

        if self.mode == RustBridgeMode::GuestWasmRpc {
            let name_lit = method.name.as_str();
            return match return_type {
                Some(return_type) => Ok(quote! {
                    pub async fn #name(&self, #(#param_defs),*) -> Result<#return_type, crate::__golem_bridge_runtime::ClientError> {
                        let result = self.#internal_name(#(#param_refs),*).await?;
                        result.ok_or_else(|| crate::__golem_bridge_runtime::ClientError::MissingResult { method: #name_lit.to_string() })
                    }
                }),
                None => Ok(quote! {
                    pub async fn #name(&self, #(#param_defs),*) -> Result<(), crate::__golem_bridge_runtime::ClientError> {
                        let _result = self.#internal_name(#(#param_refs),*).await?;
                        Ok(())
                    }
                }),
            };
        }

        match return_type {
            Some(return_type) => Ok(quote! {
                pub async fn #name(&self, #(#param_defs),*) -> Result<#return_type, crate::__golem_bridge_runtime::ClientError> {
                    let result = self.#internal_name(golem_client::model::AgentInvocationMode::Await, None, #(#param_refs),*).await?;
                    let result = result.unwrap(); // always Some because of Await
                    Ok(result)
                }
            }),
            None => Ok(quote! {
                pub async fn #name(&self, #(#param_defs),*) -> Result<(), crate::__golem_bridge_runtime::ClientError> {
                    let _result = self.#internal_name(golem_client::model::AgentInvocationMode::Await, None, #(#param_refs),*).await?;
                    Ok(())
                }
            }),
        }
    }

    fn trigger_method(&mut self, method: &AgentMethodSchema) -> anyhow::Result<TokenStream> {
        let name = self.trigger_method_ident(method);
        let internal_name = self.internal_method_ident(method);
        let param_defs = self.input_param_defs(&method.input_schema)?;
        let param_refs = self.input_param_refs(&method.input_schema)?;

        if self.mode == RustBridgeMode::GuestWasmRpc {
            let name_lit = method.name.as_str();
            let params_schema_value = self.input_param_schema_value(&method.input_schema)?;
            return Ok(quote! {
                pub fn #name(&self, #(#param_defs),*) -> Result<(), crate::__golem_bridge_runtime::ClientError> {
                    let method_parameters: crate::__golem_bridge_runtime::schema::SchemaValue = #params_schema_value;
                    let method_parameters = golem_rust::encode_schema_value(&method_parameters)
                        .map_err(|__e| crate::__golem_bridge_runtime::ClientError::SchemaEncodeFailed { message: __e.to_string() })?;
                    self.wasm_rpc.invoke(#name_lit, method_parameters)
                        .map_err(|__e| crate::__golem_bridge_runtime::ClientError::RpcFailed { message: format!("{__e:?}") })
                }
            });
        }

        Ok(quote! {
            pub async fn #name(&self, #(#param_defs),*) -> Result<(), crate::__golem_bridge_runtime::ClientError> {
                let _ = self.#internal_name(golem_client::model::AgentInvocationMode::Schedule, None, #(#param_refs),*).await?;
                Ok(())
            }
        })
    }

    fn schedule_method(&mut self, method: &AgentMethodSchema) -> anyhow::Result<TokenStream> {
        let name = self.schedule_method_ident(method);
        let internal_name = self.internal_method_ident(method);
        let param_defs = self.input_param_defs(&method.input_schema)?;
        let param_refs = self.input_param_refs(&method.input_schema)?;

        if self.mode == RustBridgeMode::GuestWasmRpc {
            let name_lit = method.name.as_str();
            let params_schema_value = self.input_param_schema_value(&method.input_schema)?;
            let scheduled_time_param = self.unique_internal_ident(
                "__golem_bridge_scheduled_time",
                &self.input_param_ident_names(&method.input_schema)?,
            );
            return Ok(quote! {
                pub fn #name(&self, #scheduled_time_param: golem_rust::wasip2::clocks::wall_clock::Datetime, #(#param_defs),*) -> Result<(), crate::__golem_bridge_runtime::ClientError> {
                    let method_parameters: crate::__golem_bridge_runtime::schema::SchemaValue = #params_schema_value;
                    let method_parameters = golem_rust::encode_schema_value(&method_parameters)
                        .map_err(|__e| crate::__golem_bridge_runtime::ClientError::SchemaEncodeFailed { message: __e.to_string() })?;
                    self.wasm_rpc.schedule_invocation(#scheduled_time_param, #name_lit, method_parameters);
                    Ok(())
                }
            });
        }

        Ok(quote! {
            pub async fn #name(&self, when: chrono::DateTime<chrono::Utc>, #(#param_defs),*) -> Result<(), crate::__golem_bridge_runtime::ClientError> {
                let _ = self.#internal_name(golem_client::model::AgentInvocationMode::Schedule, Some(when), #(#param_refs),*).await?;
                Ok(())
            }
        })
    }

    fn internal_method(&mut self, method: &AgentMethodSchema) -> anyhow::Result<TokenStream> {
        let name_lit = method.name.as_str();
        let name = self.internal_method_ident(method);
        let param_defs = self.input_param_defs(&method.input_schema)?;
        let params_value = self.input_param_value(&method.input_schema)?;
        let return_type = self.output_return_type(&method.output_schema)?;

        if self.mode == RustBridgeMode::GuestWasmRpc {
            let params_schema_value = self.input_param_schema_value(&method.input_schema)?;
            return match return_type {
                Some(return_type) => {
                    let decode_body = self.output_decode_expr(&method.output_schema)?;
                    Ok(quote! {
                        async fn #name(&self, #(#param_defs),*) -> Result<Option<#return_type>, crate::__golem_bridge_runtime::ClientError> {
                            let method_parameters: crate::__golem_bridge_runtime::schema::SchemaValue = #params_schema_value;
                            let method_parameters = golem_rust::encode_schema_value(&method_parameters)
                                .map_err(|__e| crate::__golem_bridge_runtime::ClientError::SchemaEncodeFailed { message: __e.to_string() })?;
                            let rpc_result_future = self.wasm_rpc.async_invoke_and_await(#name_lit, method_parameters);
                            let response = golem_rust::agentic::await_invoke_schema_value_result(rpc_result_future).await
                                .map_err(|__e| crate::__golem_bridge_runtime::ClientError::RpcFailed { message: format!("{__e:?}") })?;
                            match response {
                                Some(__value) => {
                                    let __decoded: #return_type = (|| -> Result<#return_type, String> {
                                        #decode_body
                                    })().map_err(|__e| crate::__golem_bridge_runtime::ClientError::SchemaDecodeFailed { message: __e })?;
                                    Ok(Some(__decoded))
                                }
                                None => Ok(None),
                            }
                        }
                    })
                }
                None => Ok(quote! {
                    async fn #name(&self, #(#param_defs),*) -> Result<Option<()>, crate::__golem_bridge_runtime::ClientError> {
                        let method_parameters: crate::__golem_bridge_runtime::schema::SchemaValue = #params_schema_value;
                        let method_parameters = golem_rust::encode_schema_value(&method_parameters)
                            .map_err(|__e| crate::__golem_bridge_runtime::ClientError::SchemaEncodeFailed { message: __e.to_string() })?;
                        let rpc_result_future = self.wasm_rpc.async_invoke_and_await(#name_lit, method_parameters);
                        let _response = golem_rust::agentic::await_invoke_schema_value_result(rpc_result_future).await
                            .map_err(|__e| crate::__golem_bridge_runtime::ClientError::RpcFailed { message: format!("{__e:?}") })?;
                        Ok(Some(()))
                    }
                }),
            };
        }

        match return_type {
            Some(return_type) => {
                let decode_body = self.output_decode_expr(&method.output_schema)?;
                Ok(quote! {
                    async fn #name(&self, mode: golem_client::model::AgentInvocationMode, when: Option<chrono::DateTime<chrono::Utc>>, #(#param_defs),*) -> Result<Option<#return_type>, crate::__golem_bridge_runtime::ClientError> {
                        let method_parameters: serde_json::Value = #params_value;
                        let response = self.invoke(#name_lit, method_parameters, mode, when).await?;
                        match response {
                            Some(__typed) => {
                                let (_, __value) = __typed.into_parts();
                                let __decoded: #return_type = (|| -> Result<#return_type, String> {
                                    #decode_body
                                })().map_err(|__e| crate::__golem_bridge_runtime::ClientError::InvocationFailed { message: format!("Failed to decode result value: {__e}") })?;
                                Ok(Some(__decoded))
                            }
                            None => Ok(None),
                        }
                    }
                })
            }
            None => Ok(quote! {
                async fn #name(&self, mode: golem_client::model::AgentInvocationMode, when: Option<chrono::DateTime<chrono::Utc>>, #(#param_defs),*) -> Result<Option<()>, crate::__golem_bridge_runtime::ClientError> {
                    let method_parameters: serde_json::Value = #params_value;
                    let response = self.invoke(#name_lit, method_parameters, mode, when).await?;
                    match response {
                        Some(_) => Ok(Some(())),
                        None => Ok(None),
                    }
                }
            }),
        }
    }

    // --- Input parameter surface -------------------------------------------

    /// `name: type` parameter definitions for a constructor or method.
    fn input_param_defs(&mut self, input: &InputSchema) -> anyhow::Result<Vec<TokenStream>> {
        let names = self.input_param_ident_names(input)?;
        self.input_param_defs_with_ident_names(input, &names)
    }

    fn input_param_defs_with_ident_names(
        &mut self,
        input: &InputSchema,
        names: &[String],
    ) -> anyhow::Result<Vec<TokenStream>> {
        match self.rust_input(input)? {
            RustInput::Params(params) => {
                if params.len() != names.len() {
                    bail!("parameter name allocation does not match input parameter count");
                }
                let mut result = Vec::new();
                for ((_, schema), name) in params.iter().zip(names) {
                    let ident = Self::ident_from_name(name);
                    let typ = self.type_reference(schema, false)?;
                    result.push(quote! { #ident: #typ });
                }
                Ok(result)
            }
            RustInput::Multimodal(cases) => {
                if names.len() != 1 {
                    bail!("multimodal input must have exactly one allocated parameter name");
                }
                let name = self.get_or_create_multimodal(&cases);
                let name = Ident::new(&name, Span::call_site());
                let param_name = Self::ident_from_name(&names[0]);
                Ok(vec![quote! { #param_name: Vec<#name> }])
            }
        }
    }

    /// Bare parameter idents for forwarding to the internal method.
    fn input_param_refs(&mut self, input: &InputSchema) -> anyhow::Result<Vec<TokenStream>> {
        let names = self.input_param_ident_names(input)?;
        Ok(self.input_param_refs_with_ident_names(&names))
    }

    fn input_param_refs_with_ident_names(&self, names: &[String]) -> Vec<TokenStream> {
        names
            .iter()
            .map(|name| {
                let ident = Self::ident_from_name(name);
                quote! { #ident }
            })
            .collect()
    }

    fn input_param_ident_names(&self, input: &InputSchema) -> anyhow::Result<Vec<String>> {
        match self.rust_input(input)? {
            RustInput::Params(params) => Ok(params
                .iter()
                .map(|(name, _)| self.to_rust_ident(name))
                .collect()),
            RustInput::Multimodal(_) => Ok(vec!["values".to_string()]),
        }
    }

    fn input_param_ident_names_unique(&self, input: &InputSchema) -> anyhow::Result<Vec<String>> {
        let mut naming = ParameterNaming::new();
        Ok(self
            .input_param_ident_names(input)?
            .into_iter()
            .map(|name| naming.fresh(name))
            .collect())
    }

    fn unique_internal_ident(&self, base: &str, occupied: &[String]) -> Ident {
        let mut candidate = self.to_rust_ident(base);
        let mut suffix = 0usize;
        while occupied.iter().any(|name| name == &candidate) {
            suffix += 1;
            candidate = self.to_rust_ident(&format!("{base}_{suffix}"));
        }
        Ident::new(&candidate, Span::call_site())
    }

    fn ident_from_name(name: impl AsRef<str>) -> Ident {
        Ident::new(name.as_ref(), Span::call_site())
    }

    /// Block expression of type `serde_json::Value` encoding the input
    /// parameters into a schema-native `record` and serializing it.
    fn input_param_value(&mut self, input: &InputSchema) -> anyhow::Result<TokenStream> {
        let schema_value = self.input_param_schema_value(input)?;

        Ok(quote! {
            {
                let __sv: crate::__golem_bridge_runtime::schema::SchemaValue = #schema_value;
                serde_json::to_value(&__sv).map_err(|__e| crate::__golem_bridge_runtime::ClientError::InvocationFailed { message: format!("Failed to serialize parameters: {__e}") })?
            }
        })
    }

    /// Block expression of type `SchemaValue` encoding the input parameters
    /// into a schema-native `record`.
    fn input_param_schema_value(&mut self, input: &InputSchema) -> anyhow::Result<TokenStream> {
        let names = self.input_param_ident_names(input)?;
        self.input_param_schema_value_with_ident_names(input, &names)
    }

    fn input_param_schema_value_with_ident_names(
        &mut self,
        input: &InputSchema,
        names: &[String],
    ) -> anyhow::Result<TokenStream> {
        let encode_error = match self.mode {
            RustBridgeMode::ExternalRest => quote! {
                crate::__golem_bridge_runtime::ClientError::InvocationFailed { message: format!("Failed to encode parameters: {__e}") }
            },
            RustBridgeMode::GuestWasmRpc => quote! {
                crate::__golem_bridge_runtime::ClientError::SchemaEncodeFailed { message: format!("Failed to encode parameters: {__e}") }
            },
        };
        let record_body = match self.rust_input(input)? {
            RustInput::Params(params) => {
                if params.len() != names.len() {
                    bail!("parameter name allocation does not match input parameter count");
                }
                let mut field_encs = Vec::new();
                for ((_, schema), name) in params.iter().zip(names) {
                    let ident = Self::ident_from_name(name);
                    let enc = self.emit_encode_expr(quote! { #ident }, schema, false, 0)?;
                    field_encs.push(quote! { #enc? });
                }
                quote! {
                    Ok(crate::__golem_bridge_runtime::schema::SchemaValue::Record { fields: vec![#(#field_encs),*] })
                }
            }
            RustInput::Multimodal(cases) => {
                if names.len() != 1 {
                    bail!("multimodal input must have exactly one allocated parameter name");
                }
                let param_name = Self::ident_from_name(&names[0]);
                let list = self.multimodal_list_encode(&cases, quote! { #param_name })?;
                quote! {
                    Ok(crate::__golem_bridge_runtime::schema::SchemaValue::Record { fields: vec![#list?] })
                }
            }
        };

        Ok(quote! {
            {
                (|| -> Result<crate::__golem_bridge_runtime::schema::SchemaValue, String> {
                    #record_body
                })().map_err(|__e| #encode_error)?
            }
        })
    }

    // --- Output surface -----------------------------------------------------

    fn output_return_type(&mut self, output: &OutputSchema) -> anyhow::Result<Option<TokenStream>> {
        match self.rust_output(output)? {
            RustOutput::Unit => Ok(None),
            RustOutput::Single(schema) => Ok(Some(self.type_reference(&schema, false)?)),
            RustOutput::Multimodal(cases) => {
                let name = self.get_or_create_multimodal(&cases);
                let name = Ident::new(&name, Span::call_site());
                Ok(Some(quote! { Vec<#name> }))
            }
        }
    }

    /// Expression of type `Result<ReturnType, String>` decoding the bare output
    /// `SchemaValue` (`__value`) into the method return value.
    fn output_decode_expr(&mut self, output: &OutputSchema) -> anyhow::Result<TokenStream> {
        match self.rust_output(output)? {
            RustOutput::Unit => Ok(quote! { Ok(()) }),
            RustOutput::Single(schema) => {
                self.emit_decode_expr(quote! { __value }, &schema, false, 0)
            }
            RustOutput::Multimodal(cases) => {
                let decode = self.multimodal_list_decode(&cases, quote! { __value })?;
                Ok(decode)
            }
        }
    }

    // --- Multimodal ---------------------------------------------------------

    fn get_or_create_multimodal(&mut self, cases: &[(String, SchemaType)]) -> String {
        if let Some((_, name)) = self.known_multimodals.iter().find(|(c, _)| c == cases) {
            return name.clone();
        }
        let name = format!("Multimodal{}", self.known_multimodals.len());
        self.known_multimodals.push((cases.to_vec(), name.clone()));
        name
    }

    /// `Result<SchemaValue, String>` expression encoding a `Vec<MultimodalN>`
    /// (`values_expr`) into a `list<variant<…>>`.
    fn multimodal_list_encode(
        &mut self,
        cases: &[(String, SchemaType)],
        values_expr: TokenStream,
    ) -> anyhow::Result<TokenStream> {
        let name = self.get_or_create_multimodal(cases);
        let encode_fn = Ident::new(&format!("encode_{name}"), Span::call_site());
        Ok(quote! {
            #values_expr
                .into_iter()
                .map(|__m| #encode_fn(__m))
                .collect::<Result<Vec<crate::__golem_bridge_runtime::schema::SchemaValue>, String>>()
                .map(|__elems| crate::__golem_bridge_runtime::schema::SchemaValue::List { elements: __elems })
        })
    }

    /// `Result<Vec<MultimodalN>, String>` expression decoding a `list<variant<…>>`
    /// (`value_expr`) into a `Vec<MultimodalN>`.
    fn multimodal_list_decode(
        &mut self,
        cases: &[(String, SchemaType)],
        value_expr: TokenStream,
    ) -> anyhow::Result<TokenStream> {
        let name = self.get_or_create_multimodal(cases);
        let enum_ident = Ident::new(&name, Span::call_site());
        let decode_fn = Ident::new(&format!("decode_{name}"), Span::call_site());
        Ok(quote! {
            match #value_expr {
                crate::__golem_bridge_runtime::schema::SchemaValue::List { elements } => {
                    elements.into_iter().map(|__e| #decode_fn(__e)).collect::<Result<Vec<#enum_ident>, String>>()
                }
                __other => Err(format!("Expected a multimodal list value, got {:?}", __other)),
            }
        })
    }

    fn multimodals(&mut self) -> anyhow::Result<TokenStream> {
        if self.known_multimodals.is_empty() {
            return Ok(quote! {});
        }

        let mut items = Vec::new();
        for (cases, name) in self.known_multimodals.clone() {
            let enum_ident = Ident::new(&name, Span::call_site());

            let mut variants = Vec::new();
            let mut encode_arms = Vec::new();
            let mut decode_arms = Vec::new();

            for (idx, (case_name, payload)) in cases.iter().enumerate() {
                let case_ident = Ident::new(&self.to_rust_case_name(case_name), Span::call_site());
                let payload_type = self.type_reference(payload, false)?;
                variants.push(quote! { #case_ident(#payload_type) });

                let idx_u32 = idx as u32;
                let enc = self.emit_encode_expr(quote! { __inner }, payload, false, 0)?;
                encode_arms.push(quote! {
                    #enum_ident::#case_ident(__inner) => Ok(crate::__golem_bridge_runtime::schema::SchemaValue::Variant(crate::__golem_bridge_runtime::schema::VariantValuePayload {
                        case: #idx_u32,
                        payload: Some(Box::new(#enc?)),
                    })),
                });

                let dec = self.emit_decode_expr(quote! { __pv }, payload, false, 0)?;
                decode_arms.push(quote! {
                    #idx_u32 => {
                        let __p = payload.ok_or_else(|| format!("Missing multimodal payload for case {}", #case_name))?;
                        let __pv = *__p;
                        Ok(#enum_ident::#case_ident(#dec?))
                    }
                });
            }

            let encode_fn = Ident::new(&format!("encode_{name}"), Span::call_site());
            let decode_fn = Ident::new(&format!("decode_{name}"), Span::call_site());

            items.push(quote! {
                #[derive(Debug, Clone)]
                pub enum #enum_ident {
                    #(#variants),*
                }

                fn #encode_fn(value: #enum_ident) -> Result<crate::__golem_bridge_runtime::schema::SchemaValue, String> {
                    match value {
                        #(#encode_arms)*
                    }
                }

                fn #decode_fn(value: crate::__golem_bridge_runtime::schema::SchemaValue) -> Result<#enum_ident, String> {
                    match value {
                        crate::__golem_bridge_runtime::schema::SchemaValue::Variant(crate::__golem_bridge_runtime::schema::VariantValuePayload { case, payload }) => match case {
                            #(#decode_arms)*
                            __other => Err(format!("Invalid multimodal variant case index: {}", __other)),
                        },
                        __other => Err(format!("Expected variant value, got {:?}", __other)),
                    }
                }
            });
        }

        Ok(quote! {
            #(#items)*
        })
    }

    // --- Type definitions + codecs -----------------------------------------

    fn type_definitions(&mut self) -> anyhow::Result<TokenStream> {
        let types: Vec<(SchemaType, RustTypeName)> = self
            .type_naming
            .types()
            .map(|(t, n)| (t.clone(), n.clone()))
            .collect();

        let mut defs = Vec::new();
        for (typ, name) in &types {
            let RustTypeName::Derived(name_str) = name else {
                bail!("Remapped type names are not supported yet");
            };
            let name_ident = Ident::new(name_str, Span::call_site());
            let resolved = self.resolve_ref_owned(typ);

            let typedef = self.emit_typedef(&name_ident, &resolved)?;
            let encode_fn = Ident::new(&format!("encode_{name_str}"), Span::call_site());
            let decode_fn = Ident::new(&format!("decode_{name_str}"), Span::call_site());
            let encode_body = self.emit_encode_body(&name_ident, &resolved)?;
            let decode_body = self.emit_decode_body(&name_ident, &resolved)?;

            defs.push(quote! {
                #typedef

                fn #encode_fn(value: #name_ident) -> Result<crate::__golem_bridge_runtime::schema::SchemaValue, String> {
                    #encode_body
                }

                fn #decode_fn(value: crate::__golem_bridge_runtime::schema::SchemaValue) -> Result<#name_ident, String> {
                    #decode_body
                }
            });
        }

        Ok(quote! { #(#defs)* })
    }

    /// Emit the `pub struct` / `pub enum` / `pub type` definition for a named
    /// type, given its already-resolved body.
    fn emit_typedef(&mut self, name: &Ident, resolved: &SchemaType) -> anyhow::Result<TokenStream> {
        match resolved {
            SchemaType::Record { fields, .. } => {
                let mut emitted = Vec::new();
                for field in fields {
                    let ident = Ident::new(&self.to_rust_ident(&field.name), Span::call_site());
                    let ty = self.type_reference(&field.body, true)?;
                    emitted.push(quote! { pub #ident: #ty });
                }
                Ok(quote! {
                    #[derive(Debug, Clone)]
                    pub struct #name {
                        #(#emitted),*
                    }
                })
            }
            SchemaType::Variant { cases, .. } => {
                let mut emitted = Vec::new();
                for case in cases {
                    let case_ident =
                        Ident::new(&self.to_rust_case_name(&case.name), Span::call_site());
                    match &case.payload {
                        Some(payload) => {
                            let ty = self.type_reference(payload, true)?;
                            emitted.push(quote! { #case_ident(#ty) });
                        }
                        None => emitted.push(quote! { #case_ident }),
                    }
                }
                Ok(quote! {
                    #[derive(Debug, Clone)]
                    pub enum #name {
                        #(#emitted),*
                    }
                })
            }
            SchemaType::Enum { cases, .. } => {
                let emitted = cases
                    .iter()
                    .map(|c| {
                        let ident = Ident::new(&self.to_rust_case_name(c), Span::call_site());
                        quote! { #ident }
                    })
                    .collect::<Vec<_>>();
                Ok(quote! {
                    #[derive(Debug, Clone)]
                    pub enum #name {
                        #(#emitted),*
                    }
                })
            }
            SchemaType::Flags { flags, .. } => {
                let emitted = flags
                    .iter()
                    .map(|f| {
                        let ident = Ident::new(&self.to_rust_ident(f), Span::call_site());
                        quote! { pub #ident: bool }
                    })
                    .collect::<Vec<_>>();
                Ok(quote! {
                    #[derive(Debug, Clone)]
                    pub struct #name {
                        #(#emitted),*
                    }
                })
            }
            SchemaType::Union { spec, .. } => {
                let mut emitted = Vec::new();
                for branch in &spec.branches {
                    let branch_ident =
                        Ident::new(&self.to_rust_case_name(&branch.tag), Span::call_site());
                    let ty = self.type_reference(&branch.body, true)?;
                    emitted.push(quote! { #branch_ident(#ty) });
                }
                Ok(quote! {
                    #[derive(Debug, Clone)]
                    pub enum #name {
                        #(#emitted),*
                    }
                })
            }
            other => {
                // Aliases to structural / scalar forms.
                let ty = self.type_reference(other, true)?;
                Ok(quote! { pub type #name = #ty; })
            }
        }
    }

    /// Body of `encode_<Name>` given the resolved type body. The generated
    /// `encode_<Name>` function binds its argument as `value`.
    fn emit_encode_body(
        &mut self,
        name: &Ident,
        resolved: &SchemaType,
    ) -> anyhow::Result<TokenStream> {
        match resolved {
            SchemaType::Record { fields, .. } => {
                let field_idents = fields
                    .iter()
                    .map(|f| Ident::new(&self.to_rust_ident(&f.name), Span::call_site()))
                    .collect::<Vec<_>>();
                let mut field_encs = Vec::new();
                for (field, ident) in fields.iter().zip(&field_idents) {
                    let enc = self.emit_encode_expr(quote! { #ident }, &field.body, true, 0)?;
                    field_encs.push(quote! { #enc? });
                }
                Ok(quote! {
                    let #name { #(#field_idents),* } = value;
                    Ok(crate::__golem_bridge_runtime::schema::SchemaValue::Record { fields: vec![#(#field_encs),*] })
                })
            }
            SchemaType::Variant { cases, .. } => {
                let mut arms = Vec::new();
                for (idx, case) in cases.iter().enumerate() {
                    let case_ident =
                        Ident::new(&self.to_rust_case_name(&case.name), Span::call_site());
                    let idx_u32 = idx as u32;
                    match &case.payload {
                        Some(payload) => {
                            let enc =
                                self.emit_encode_expr(quote! { __inner }, payload, true, 0)?;
                            arms.push(quote! {
                                #name::#case_ident(__inner) => Ok(crate::__golem_bridge_runtime::schema::SchemaValue::Variant(crate::__golem_bridge_runtime::schema::VariantValuePayload {
                                    case: #idx_u32,
                                    payload: Some(Box::new(#enc?)),
                                })),
                            });
                        }
                        None => arms.push(quote! {
                            #name::#case_ident => Ok(crate::__golem_bridge_runtime::schema::SchemaValue::Variant(crate::__golem_bridge_runtime::schema::VariantValuePayload {
                                case: #idx_u32,
                                payload: None,
                            })),
                        }),
                    }
                }
                Ok(quote! {
                    match value {
                        #(#arms)*
                    }
                })
            }
            SchemaType::Enum { cases, .. } => {
                let mut arms = Vec::new();
                for (idx, case) in cases.iter().enumerate() {
                    let case_ident = Ident::new(&self.to_rust_case_name(case), Span::call_site());
                    let idx_u32 = idx as u32;
                    arms.push(quote! {
                        #name::#case_ident => Ok(crate::__golem_bridge_runtime::schema::SchemaValue::Enum { case: #idx_u32 }),
                    });
                }
                Ok(quote! {
                    match value {
                        #(#arms)*
                    }
                })
            }
            SchemaType::Flags { flags, .. } => {
                let flag_idents = flags
                    .iter()
                    .map(|f| Ident::new(&self.to_rust_ident(f), Span::call_site()))
                    .collect::<Vec<_>>();
                Ok(quote! {
                    let #name { #(#flag_idents),* } = value;
                    Ok(crate::__golem_bridge_runtime::schema::SchemaValue::Flags { bits: vec![#(#flag_idents),*] })
                })
            }
            SchemaType::Union { spec, .. } => {
                let mut arms = Vec::new();
                for branch in &spec.branches {
                    let branch_ident =
                        Ident::new(&self.to_rust_case_name(&branch.tag), Span::call_site());
                    let tag = branch.tag.as_str();
                    let enc = self.emit_encode_expr(quote! { __inner }, &branch.body, true, 0)?;
                    arms.push(quote! {
                        #name::#branch_ident(__inner) => Ok(crate::__golem_bridge_runtime::schema::SchemaValue::Union(crate::__golem_bridge_runtime::schema::UnionValuePayload {
                            tag: #tag.to_string(),
                            body: Box::new(#enc?),
                        })),
                    });
                }
                Ok(quote! {
                    match value {
                        #(#arms)*
                    }
                })
            }
            other => self.emit_encode_structural(quote! { value }, other, true, 0),
        }
    }

    /// Body of `decode_<Name>` given the resolved type body. The generated
    /// `decode_<Name>` function binds its argument as `value`.
    fn emit_decode_body(
        &mut self,
        name: &Ident,
        resolved: &SchemaType,
    ) -> anyhow::Result<TokenStream> {
        match resolved {
            SchemaType::Record { fields, .. } => {
                let n = fields.len();
                let mut field_inits = Vec::new();
                for field in fields {
                    let ident = Ident::new(&self.to_rust_ident(&field.name), Span::call_site());
                    let dec = self.emit_decode_expr(
                        quote! { __it.next().unwrap() },
                        &field.body,
                        true,
                        0,
                    )?;
                    field_inits.push(quote! { #ident: #dec? });
                }
                Ok(quote! {
                    match value {
                        crate::__golem_bridge_runtime::schema::SchemaValue::Record { fields } => {
                            if fields.len() != #n {
                                return Err(format!("Expected record with {} fields, got {}", #n, fields.len()));
                            }
                            let mut __it = fields.into_iter();
                            Ok(#name { #(#field_inits),* })
                        }
                        __other => Err(format!("Expected record value, got {:?}", __other)),
                    }
                })
            }
            SchemaType::Variant { cases, .. } => {
                let mut arms = Vec::new();
                for (idx, case) in cases.iter().enumerate() {
                    let case_ident =
                        Ident::new(&self.to_rust_case_name(&case.name), Span::call_site());
                    let idx_u32 = idx as u32;
                    match &case.payload {
                        Some(payload) => {
                            let dec = self.emit_decode_expr(quote! { __pv }, payload, true, 0)?;
                            arms.push(quote! {
                                #idx_u32 => {
                                    let __p = payload.ok_or_else(|| format!("Missing payload for variant case {}", #idx_u32))?;
                                    let __pv = *__p;
                                    Ok(#name::#case_ident(#dec?))
                                }
                            });
                        }
                        None => arms.push(quote! {
                            #idx_u32 => Ok(#name::#case_ident),
                        }),
                    }
                }
                Ok(quote! {
                    match value {
                        crate::__golem_bridge_runtime::schema::SchemaValue::Variant(crate::__golem_bridge_runtime::schema::VariantValuePayload { case, payload }) => match case {
                            #(#arms)*
                            __other => Err(format!("Invalid variant case index: {}", __other)),
                        },
                        __other => Err(format!("Expected variant value, got {:?}", __other)),
                    }
                })
            }
            SchemaType::Enum { cases, .. } => {
                let mut arms = Vec::new();
                for (idx, case) in cases.iter().enumerate() {
                    let case_ident = Ident::new(&self.to_rust_case_name(case), Span::call_site());
                    let idx_u32 = idx as u32;
                    arms.push(quote! { #idx_u32 => Ok(#name::#case_ident), });
                }
                Ok(quote! {
                    match value {
                        crate::__golem_bridge_runtime::schema::SchemaValue::Enum { case } => match case {
                            #(#arms)*
                            __other => Err(format!("Invalid enum case index: {}", __other)),
                        },
                        __other => Err(format!("Expected enum value, got {:?}", __other)),
                    }
                })
            }
            SchemaType::Flags { flags, .. } => {
                let n = flags.len();
                let mut field_inits = Vec::new();
                for (idx, flag) in flags.iter().enumerate() {
                    let ident = Ident::new(&self.to_rust_ident(flag), Span::call_site());
                    field_inits.push(quote! { #ident: bits[#idx] });
                }
                Ok(quote! {
                    match value {
                        crate::__golem_bridge_runtime::schema::SchemaValue::Flags { bits } => {
                            if bits.len() != #n {
                                return Err(format!("Expected flags with {} bits, got {}", #n, bits.len()));
                            }
                            Ok(#name { #(#field_inits),* })
                        }
                        __other => Err(format!("Expected flags value, got {:?}", __other)),
                    }
                })
            }
            SchemaType::Union { spec, .. } => {
                let mut arms = Vec::new();
                for branch in &spec.branches {
                    let branch_ident =
                        Ident::new(&self.to_rust_case_name(&branch.tag), Span::call_site());
                    let tag = branch.tag.as_str();
                    let dec = self.emit_decode_expr(quote! { __bv }, &branch.body, true, 0)?;
                    arms.push(quote! {
                        #tag => Ok(#name::#branch_ident(#dec?)),
                    });
                }
                Ok(quote! {
                    match value {
                        crate::__golem_bridge_runtime::schema::SchemaValue::Union(crate::__golem_bridge_runtime::schema::UnionValuePayload { tag, body }) => {
                            let __bv = *body;
                            match tag.as_str() {
                                #(#arms)*
                                __other => Err(format!("Unknown union branch tag: {}", __other)),
                            }
                        }
                        __other => Err(format!("Expected union value, got {:?}", __other)),
                    }
                })
            }
            other => self.emit_decode_structural(quote! { value }, other, true, 0),
        }
    }

    // --- Codec dispatchers --------------------------------------------------

    /// `Result<SchemaValue, String>` expression encoding `val` (a value of the
    /// Rust type for `typ`). Named types delegate to their `encode_<Name>`
    /// function; everything else is encoded inline.
    fn emit_encode_expr(
        &mut self,
        val: TokenStream,
        typ: &SchemaType,
        box_recursive: bool,
        depth: usize,
    ) -> anyhow::Result<TokenStream> {
        let inner = if let Some(name) = self.type_naming.type_name_for_type(typ).cloned() {
            let RustTypeName::Derived(n) = name else {
                bail!("Remapped type names are not supported yet");
            };
            let encode_fn = Ident::new(&format!("encode_{n}"), Span::call_site());
            if box_recursive && self.type_naming.is_recursive_ref(typ) {
                quote! { #encode_fn(*#val) }
            } else {
                quote! { #encode_fn(#val) }
            }
        } else {
            self.emit_encode_structural(val, typ, box_recursive, depth)?
        };
        // Pin the error type to `String` so callers can apply `?` to the
        // expression regardless of context (a bare `Ok(..)` leaf would
        // otherwise leave the error type unconstrained).
        Ok(quote! { { let __r: Result<_, String> = { #inner }; __r } })
    }

    /// `Result<RustType, String>` expression decoding `val` (a `SchemaValue`)
    /// into the Rust type for `typ`.
    fn emit_decode_expr(
        &mut self,
        val: TokenStream,
        typ: &SchemaType,
        box_recursive: bool,
        depth: usize,
    ) -> anyhow::Result<TokenStream> {
        let inner = if let Some(name) = self.type_naming.type_name_for_type(typ).cloned() {
            let RustTypeName::Derived(n) = name else {
                bail!("Remapped type names are not supported yet");
            };
            let decode_fn = Ident::new(&format!("decode_{n}"), Span::call_site());
            if box_recursive && self.type_naming.is_recursive_ref(typ) {
                quote! { #decode_fn(#val).map(Box::new) }
            } else {
                quote! { #decode_fn(#val) }
            }
        } else {
            self.emit_decode_structural(val, typ, box_recursive, depth)?
        };
        // Pin the error type to `String` so callers can apply `?` to the
        // expression regardless of context.
        Ok(quote! { { let __r: Result<_, String> = { #inner }; __r } })
    }

    fn emit_encode_structural(
        &mut self,
        val: TokenStream,
        typ: &SchemaType,
        box_recursive: bool,
        depth: usize,
    ) -> anyhow::Result<TokenStream> {
        // Role-marked unstructured-text/binary variant → ergonomic wrapper.
        let text_restrictions = {
            let graph = self.type_naming.graph();
            unstructured_text_restrictions(graph, typ)?.cloned()
        };
        if let Some(restrictions) = text_restrictions {
            let ty = self.unstructured_text_type(&restrictions);
            return Ok(match self.mode {
                RustBridgeMode::ExternalRest => quote! { #val.to_schema_value() },
                RustBridgeMode::GuestWasmRpc => {
                    quote! { <#ty as crate::__golem_bridge_runtime::agentic::Schema>::to_schema_value(#val) }
                }
            });
        }
        let binary_restrictions = {
            let graph = self.type_naming.graph();
            unstructured_binary_restrictions(graph, typ)?.cloned()
        };
        if let Some(restrictions) = binary_restrictions {
            let ty = self.unstructured_binary_type(&restrictions);
            return Ok(match self.mode {
                RustBridgeMode::ExternalRest => quote! { #val.to_schema_value() },
                RustBridgeMode::GuestWasmRpc => {
                    quote! { <#ty as crate::__golem_bridge_runtime::agentic::Schema>::to_schema_value(#val) }
                }
            });
        }
        let e = Ident::new(&format!("__e{depth}"), Span::call_site());
        let next = depth + 1;
        let rendered = match typ {
            SchemaType::Bool { .. } => {
                quote! { Ok(crate::__golem_bridge_runtime::schema::SchemaValue::Bool(#val)) }
            }
            SchemaType::S8 { .. } => {
                quote! { Ok(crate::__golem_bridge_runtime::schema::SchemaValue::S8(#val)) }
            }
            SchemaType::S16 { .. } => {
                quote! { Ok(crate::__golem_bridge_runtime::schema::SchemaValue::S16(#val)) }
            }
            SchemaType::S32 { .. } => {
                quote! { Ok(crate::__golem_bridge_runtime::schema::SchemaValue::S32(#val)) }
            }
            SchemaType::S64 { .. } => {
                quote! { Ok(crate::__golem_bridge_runtime::schema::SchemaValue::S64(#val)) }
            }
            SchemaType::U8 { .. } => {
                quote! { Ok(crate::__golem_bridge_runtime::schema::SchemaValue::U8(#val)) }
            }
            SchemaType::U16 { .. } => {
                quote! { Ok(crate::__golem_bridge_runtime::schema::SchemaValue::U16(#val)) }
            }
            SchemaType::U32 { .. } => {
                quote! { Ok(crate::__golem_bridge_runtime::schema::SchemaValue::U32(#val)) }
            }
            SchemaType::U64 { .. } => {
                quote! { Ok(crate::__golem_bridge_runtime::schema::SchemaValue::U64(#val)) }
            }
            SchemaType::F32 { .. } => {
                quote! { Ok(crate::__golem_bridge_runtime::schema::SchemaValue::F32(#val)) }
            }
            SchemaType::F64 { .. } => {
                quote! { Ok(crate::__golem_bridge_runtime::schema::SchemaValue::F64(#val)) }
            }
            SchemaType::Char { .. } => {
                quote! { Ok(crate::__golem_bridge_runtime::schema::SchemaValue::Char(#val)) }
            }
            SchemaType::String { .. } => {
                quote! { Ok(crate::__golem_bridge_runtime::schema::SchemaValue::String(#val)) }
            }
            SchemaType::Option { inner, .. } => {
                let inner_enc = self.emit_encode_expr(quote! { #e }, inner, box_recursive, next)?;
                quote! {
                    match #val {
                        Some(#e) => Ok(crate::__golem_bridge_runtime::schema::SchemaValue::Option { inner: Some(Box::new(#inner_enc?)) }),
                        None => Ok(crate::__golem_bridge_runtime::schema::SchemaValue::Option { inner: None }),
                    }
                }
            }
            SchemaType::List { element, .. } => {
                let inner_enc = self.emit_encode_expr(quote! { #e }, element, false, next)?;
                quote! {
                    #val
                        .into_iter()
                        .map(|#e| #inner_enc)
                        .collect::<Result<Vec<crate::__golem_bridge_runtime::schema::SchemaValue>, String>>()
                        .map(|__elems| crate::__golem_bridge_runtime::schema::SchemaValue::List { elements: __elems })
                }
            }
            SchemaType::FixedList {
                element, length, ..
            } => {
                let inner_enc = self.emit_encode_expr(quote! { #e }, element, false, next)?;
                let len = *length as usize;
                quote! {
                    {
                        let __elems = #val
                            .into_iter()
                            .map(|#e| #inner_enc)
                            .collect::<Result<Vec<crate::__golem_bridge_runtime::schema::SchemaValue>, String>>()?;
                        if __elems.len() != #len {
                            Err(format!("Expected fixed-list of length {}, got {}", #len, __elems.len()))
                        } else {
                            Ok(crate::__golem_bridge_runtime::schema::SchemaValue::FixedList { elements: __elems })
                        }
                    }
                }
            }
            SchemaType::Map { key, value, .. } => {
                let k = Ident::new(&format!("__k{depth}"), Span::call_site());
                let v = Ident::new(&format!("__v{depth}"), Span::call_site());
                let key_enc = self.emit_encode_expr(quote! { #k }, key, false, next)?;
                let val_enc = self.emit_encode_expr(quote! { #v }, value, false, next)?;
                quote! {
                    #val
                        .into_iter()
                        .map(|(#k, #v)| Ok::<(crate::__golem_bridge_runtime::schema::SchemaValue, crate::__golem_bridge_runtime::schema::SchemaValue), String>((#key_enc?, #val_enc?)))
                        .collect::<Result<Vec<(crate::__golem_bridge_runtime::schema::SchemaValue, crate::__golem_bridge_runtime::schema::SchemaValue)>, String>>()
                        .map(|__entries| crate::__golem_bridge_runtime::schema::SchemaValue::Map { entries: __entries })
                }
            }
            SchemaType::Tuple { elements, .. } => {
                let t = Ident::new(&format!("__t{depth}"), Span::call_site());
                let mut parts = Vec::new();
                for (idx, item) in elements.iter().enumerate() {
                    let index = Index::from(idx);
                    let enc =
                        self.emit_encode_expr(quote! { #t.#index }, item, box_recursive, next)?;
                    parts.push(quote! { #enc? });
                }
                quote! {
                    {
                        let #t = #val;
                        Ok(crate::__golem_bridge_runtime::schema::SchemaValue::Tuple { elements: vec![#(#parts),*] })
                    }
                }
            }
            SchemaType::Result { spec, .. } => {
                let ok_arm = match spec.ok.as_deref() {
                    Some(ok_type) => {
                        let enc =
                            self.emit_encode_expr(quote! { __r }, ok_type, box_recursive, next)?;
                        quote! { Ok(__r) => Ok(crate::__golem_bridge_runtime::schema::SchemaValue::Result(crate::__golem_bridge_runtime::schema::ResultValuePayload::Ok { value: Some(Box::new(#enc?)) })), }
                    }
                    None => {
                        quote! { Ok(_) => Ok(crate::__golem_bridge_runtime::schema::SchemaValue::Result(crate::__golem_bridge_runtime::schema::ResultValuePayload::Ok { value: None })), }
                    }
                };
                let err_arm = match spec.err.as_deref() {
                    Some(err_type) => {
                        let enc =
                            self.emit_encode_expr(quote! { __r }, err_type, box_recursive, next)?;
                        quote! { Err(__r) => Ok(crate::__golem_bridge_runtime::schema::SchemaValue::Result(crate::__golem_bridge_runtime::schema::ResultValuePayload::Err { value: Some(Box::new(#enc?)) })), }
                    }
                    None => {
                        quote! { Err(_) => Ok(crate::__golem_bridge_runtime::schema::SchemaValue::Result(crate::__golem_bridge_runtime::schema::ResultValuePayload::Err { value: None })), }
                    }
                };
                quote! {
                    match #val {
                        #ok_arm
                        #err_arm
                    }
                }
            }
            SchemaType::Text { .. } | SchemaType::Binary { .. } => {
                bail!(
                    "Bare text/binary rich scalars have no Rust bridge surface; \
                     wrap them in the unstructured text/binary variant (type = {typ:?})"
                )
            }
            SchemaType::Path { .. } => {
                quote! { Ok(crate::__golem_bridge_runtime::schema::SchemaValue::Path { path: #val }) }
            }
            SchemaType::Url { .. } => {
                quote! { Ok(crate::__golem_bridge_runtime::schema::SchemaValue::Url { url: #val }) }
            }
            SchemaType::Datetime { .. } => {
                quote! {
                    chrono::DateTime::parse_from_rfc3339(&#val)
                        .map(|__dt| crate::__golem_bridge_runtime::schema::SchemaValue::Datetime { value: __dt.with_timezone(&chrono::Utc) })
                        .map_err(|__err| format!("Invalid RFC3339 datetime: {__err}"))
                }
            }
            SchemaType::Duration { .. } => {
                quote! { Ok(crate::__golem_bridge_runtime::schema::SchemaValue::Duration(crate::__golem_bridge_runtime::schema::DurationValuePayload { nanoseconds: #val })) }
            }
            SchemaType::Ref { .. }
            | SchemaType::Record { .. }
            | SchemaType::Variant { .. }
            | SchemaType::Enum { .. }
            | SchemaType::Flags { .. }
            | SchemaType::Union { .. } => {
                bail!("Expected a generated type name for {typ:?} during encoding")
            }
            SchemaType::Quantity { .. }
            | SchemaType::Secret { .. }
            | SchemaType::QuotaToken { .. }
            | SchemaType::Future { .. }
            | SchemaType::Stream { .. } => {
                bail!("SchemaType variant has no Rust bridge encoding yet; type = {typ:?}")
            }
        };
        Ok(rendered)
    }

    fn emit_decode_structural(
        &mut self,
        val: TokenStream,
        typ: &SchemaType,
        box_recursive: bool,
        depth: usize,
    ) -> anyhow::Result<TokenStream> {
        // Role-marked unstructured-text/binary variant → ergonomic wrapper.
        let text_restrictions = {
            let graph = self.type_naming.graph();
            unstructured_text_restrictions(graph, typ)?.cloned()
        };
        if let Some(restrictions) = text_restrictions {
            let ty = self.unstructured_text_type(&restrictions);
            return Ok(match self.mode {
                RustBridgeMode::ExternalRest => quote! { <#ty>::from_schema_value(#val) },
                RustBridgeMode::GuestWasmRpc => quote! {
                    <#ty as crate::__golem_bridge_runtime::agentic::Schema>::from_schema_value(
                        #val,
                        <#ty as crate::__golem_bridge_runtime::agentic::Schema>::get_type(),
                    )
                },
            });
        }
        let binary_restrictions = {
            let graph = self.type_naming.graph();
            unstructured_binary_restrictions(graph, typ)?.cloned()
        };
        if let Some(restrictions) = binary_restrictions {
            let ty = self.unstructured_binary_type(&restrictions);
            return Ok(match self.mode {
                RustBridgeMode::ExternalRest => quote! { <#ty>::from_schema_value(#val) },
                RustBridgeMode::GuestWasmRpc => quote! {
                    <#ty as crate::__golem_bridge_runtime::agentic::Schema>::from_schema_value(
                        #val,
                        <#ty as crate::__golem_bridge_runtime::agentic::Schema>::get_type(),
                    )
                },
            });
        }
        let e = Ident::new(&format!("__e{depth}"), Span::call_site());
        let next = depth + 1;
        let rendered = match typ {
            SchemaType::Bool { .. } => quote! {
                match #val { crate::__golem_bridge_runtime::schema::SchemaValue::Bool(__b) => Ok(__b), __other => Err(format!("Expected bool value, got {:?}", __other)) }
            },
            SchemaType::S8 { .. } => quote! {
                match #val { crate::__golem_bridge_runtime::schema::SchemaValue::S8(__b) => Ok(__b), __other => Err(format!("Expected s8 value, got {:?}", __other)) }
            },
            SchemaType::S16 { .. } => quote! {
                match #val { crate::__golem_bridge_runtime::schema::SchemaValue::S16(__b) => Ok(__b), __other => Err(format!("Expected s16 value, got {:?}", __other)) }
            },
            SchemaType::S32 { .. } => quote! {
                match #val { crate::__golem_bridge_runtime::schema::SchemaValue::S32(__b) => Ok(__b), __other => Err(format!("Expected s32 value, got {:?}", __other)) }
            },
            SchemaType::S64 { .. } => quote! {
                match #val { crate::__golem_bridge_runtime::schema::SchemaValue::S64(__b) => Ok(__b), __other => Err(format!("Expected s64 value, got {:?}", __other)) }
            },
            SchemaType::U8 { .. } => quote! {
                match #val { crate::__golem_bridge_runtime::schema::SchemaValue::U8(__b) => Ok(__b), __other => Err(format!("Expected u8 value, got {:?}", __other)) }
            },
            SchemaType::U16 { .. } => quote! {
                match #val { crate::__golem_bridge_runtime::schema::SchemaValue::U16(__b) => Ok(__b), __other => Err(format!("Expected u16 value, got {:?}", __other)) }
            },
            SchemaType::U32 { .. } => quote! {
                match #val { crate::__golem_bridge_runtime::schema::SchemaValue::U32(__b) => Ok(__b), __other => Err(format!("Expected u32 value, got {:?}", __other)) }
            },
            SchemaType::U64 { .. } => quote! {
                match #val { crate::__golem_bridge_runtime::schema::SchemaValue::U64(__b) => Ok(__b), __other => Err(format!("Expected u64 value, got {:?}", __other)) }
            },
            SchemaType::F32 { .. } => quote! {
                match #val { crate::__golem_bridge_runtime::schema::SchemaValue::F32(__b) => Ok(__b), __other => Err(format!("Expected f32 value, got {:?}", __other)) }
            },
            SchemaType::F64 { .. } => quote! {
                match #val { crate::__golem_bridge_runtime::schema::SchemaValue::F64(__b) => Ok(__b), __other => Err(format!("Expected f64 value, got {:?}", __other)) }
            },
            SchemaType::Char { .. } => quote! {
                match #val { crate::__golem_bridge_runtime::schema::SchemaValue::Char(__b) => Ok(__b), __other => Err(format!("Expected char value, got {:?}", __other)) }
            },
            SchemaType::String { .. } => quote! {
                match #val { crate::__golem_bridge_runtime::schema::SchemaValue::String(__b) => Ok(__b), __other => Err(format!("Expected string value, got {:?}", __other)) }
            },
            SchemaType::Option { inner, .. } => {
                let inner_dec = self.emit_decode_expr(quote! { #e }, inner, box_recursive, next)?;
                quote! {
                    match #val {
                        crate::__golem_bridge_runtime::schema::SchemaValue::Option { inner } => match inner {
                            Some(__bx) => { let #e = *__bx; Ok(Some(#inner_dec?)) },
                            None => Ok(None),
                        },
                        __other => Err(format!("Expected option value, got {:?}", __other)),
                    }
                }
            }
            SchemaType::List { element, .. } => {
                let inner_dec = self.emit_decode_expr(quote! { #e }, element, false, next)?;
                quote! {
                    match #val {
                        crate::__golem_bridge_runtime::schema::SchemaValue::List { elements } => {
                            elements.into_iter().map(|#e| #inner_dec).collect::<Result<Vec<_>, String>>()
                        }
                        __other => Err(format!("Expected list value, got {:?}", __other)),
                    }
                }
            }
            SchemaType::FixedList {
                element, length, ..
            } => {
                let inner_dec = self.emit_decode_expr(quote! { #e }, element, false, next)?;
                let len = *length as usize;
                quote! {
                    match #val {
                        crate::__golem_bridge_runtime::schema::SchemaValue::FixedList { elements } => {
                            if elements.len() != #len {
                                return Err(format!("Expected fixed-list of length {}, got {}", #len, elements.len()));
                            }
                            elements.into_iter().map(|#e| #inner_dec).collect::<Result<Vec<_>, String>>()
                        }
                        __other => Err(format!("Expected fixed-list value, got {:?}", __other)),
                    }
                }
            }
            SchemaType::Map { key, value, .. } => {
                let k = Ident::new(&format!("__k{depth}"), Span::call_site());
                let v = Ident::new(&format!("__v{depth}"), Span::call_site());
                let key_dec = self.emit_decode_expr(quote! { #k }, key, false, next)?;
                let val_dec = self.emit_decode_expr(quote! { #v }, value, false, next)?;
                quote! {
                    match #val {
                        crate::__golem_bridge_runtime::schema::SchemaValue::Map { entries } => {
                            entries.into_iter().map(|(#k, #v)| Ok::<_, String>((#key_dec?, #val_dec?))).collect::<Result<Vec<_>, String>>()
                        }
                        __other => Err(format!("Expected map value, got {:?}", __other)),
                    }
                }
            }
            SchemaType::Tuple { elements, .. } => {
                if elements.is_empty() {
                    quote! {
                        match #val {
                            crate::__golem_bridge_runtime::schema::SchemaValue::Tuple { elements } if elements.is_empty() => Ok(()),
                            __other => Err(format!("Expected empty tuple value, got {:?}", __other)),
                        }
                    }
                } else {
                    let n = elements.len();
                    let mut parts = Vec::new();
                    for item in elements {
                        let dec = self.emit_decode_expr(
                            quote! { __it.next().unwrap() },
                            item,
                            box_recursive,
                            next,
                        )?;
                        parts.push(quote! { #dec? });
                    }
                    let tuple_expr = if parts.len() == 1 {
                        quote! { ( #(#parts),* , ) }
                    } else {
                        quote! { ( #(#parts),* ) }
                    };
                    quote! {
                        match #val {
                            crate::__golem_bridge_runtime::schema::SchemaValue::Tuple { elements } => {
                                if elements.len() != #n {
                                    return Err(format!("Expected tuple with {} elements, got {}", #n, elements.len()));
                                }
                                let mut __it = elements.into_iter();
                                Ok(#tuple_expr)
                            }
                            __other => Err(format!("Expected tuple value, got {:?}", __other)),
                        }
                    }
                }
            }
            SchemaType::Result { spec, .. } => {
                let ok_arm = match spec.ok.as_deref() {
                    Some(ok_type) => {
                        let dec =
                            self.emit_decode_expr(quote! { __ov }, ok_type, box_recursive, next)?;
                        quote! {
                            crate::__golem_bridge_runtime::schema::SchemaValue::Result(crate::__golem_bridge_runtime::schema::ResultValuePayload::Ok { value }) => {
                                let __ov = *value.ok_or_else(|| "Missing ok value".to_string())?;
                                Ok(Ok(#dec?))
                            }
                        }
                    }
                    None => quote! {
                        crate::__golem_bridge_runtime::schema::SchemaValue::Result(crate::__golem_bridge_runtime::schema::ResultValuePayload::Ok { .. }) => Ok(Ok(())),
                    },
                };
                let err_arm = match spec.err.as_deref() {
                    Some(err_type) => {
                        let dec =
                            self.emit_decode_expr(quote! { __ev }, err_type, box_recursive, next)?;
                        quote! {
                            crate::__golem_bridge_runtime::schema::SchemaValue::Result(crate::__golem_bridge_runtime::schema::ResultValuePayload::Err { value }) => {
                                let __ev = *value.ok_or_else(|| "Missing err value".to_string())?;
                                Ok(Err(#dec?))
                            }
                        }
                    }
                    None => quote! {
                        crate::__golem_bridge_runtime::schema::SchemaValue::Result(crate::__golem_bridge_runtime::schema::ResultValuePayload::Err { .. }) => Ok(Err(())),
                    },
                };
                quote! {
                    match #val {
                        #ok_arm
                        #err_arm
                        __other => Err(format!("Expected result value, got {:?}", __other)),
                    }
                }
            }
            SchemaType::Text { .. } | SchemaType::Binary { .. } => {
                bail!(
                    "Bare text/binary rich scalars have no Rust bridge surface; \
                     wrap them in the unstructured text/binary variant (type = {typ:?})"
                )
            }
            SchemaType::Path { .. } => quote! {
                match #val { crate::__golem_bridge_runtime::schema::SchemaValue::Path { path } => Ok(path), __other => Err(format!("Expected path value, got {:?}", __other)) }
            },
            SchemaType::Url { .. } => quote! {
                match #val { crate::__golem_bridge_runtime::schema::SchemaValue::Url { url } => Ok(url), __other => Err(format!("Expected url value, got {:?}", __other)) }
            },
            SchemaType::Datetime { .. } => quote! {
                match #val { crate::__golem_bridge_runtime::schema::SchemaValue::Datetime { value } => Ok(value.to_rfc3339()), __other => Err(format!("Expected datetime value, got {:?}", __other)) }
            },
            SchemaType::Duration { .. } => quote! {
                match #val { crate::__golem_bridge_runtime::schema::SchemaValue::Duration(__p) => Ok(__p.nanoseconds), __other => Err(format!("Expected duration value, got {:?}", __other)) }
            },
            SchemaType::Ref { .. }
            | SchemaType::Record { .. }
            | SchemaType::Variant { .. }
            | SchemaType::Enum { .. }
            | SchemaType::Flags { .. }
            | SchemaType::Union { .. } => {
                bail!("Expected a generated type name for {typ:?} during decoding")
            }
            SchemaType::Quantity { .. }
            | SchemaType::Secret { .. }
            | SchemaType::QuotaToken { .. }
            | SchemaType::Future { .. }
            | SchemaType::Stream { .. } => {
                bail!("SchemaType variant has no Rust bridge decoding yet; type = {typ:?}")
            }
        };
        Ok(rendered)
    }

    // --- Type references ----------------------------------------------------

    /// The Rust type referencing `typ`. Named types resolve to their generated
    /// identifier; everything else is rendered inline. `box_recursive` boxes a
    /// recursive ref in a by-value position (inside a type definition).
    fn type_reference(
        &mut self,
        typ: &SchemaType,
        box_recursive: bool,
    ) -> anyhow::Result<TokenStream> {
        if let Some(name) = self.type_naming.type_name_for_type(typ).cloned() {
            let RustTypeName::Derived(n) = name else {
                bail!("Remapped type names are not supported yet");
            };
            let ident = Ident::new(&n, Span::call_site());
            if box_recursive && self.type_naming.is_recursive_ref(typ) {
                return Ok(quote! { Box<#ident> });
            }
            return Ok(quote! { #ident });
        }

        // Role-marked unstructured-text/binary variant → ergonomic wrapper type.
        let text_restrictions = {
            let graph = self.type_naming.graph();
            unstructured_text_restrictions(graph, typ)?.cloned()
        };
        if let Some(restrictions) = text_restrictions {
            return Ok(self.unstructured_text_type(&restrictions));
        }
        let binary_restrictions = {
            let graph = self.type_naming.graph();
            unstructured_binary_restrictions(graph, typ)?.cloned()
        };
        if let Some(restrictions) = binary_restrictions {
            return Ok(self.unstructured_binary_type(&restrictions));
        }

        match typ {
            SchemaType::Bool { .. } => Ok(quote! { bool }),
            SchemaType::S8 { .. } => Ok(quote! { i8 }),
            SchemaType::S16 { .. } => Ok(quote! { i16 }),
            SchemaType::S32 { .. } => Ok(quote! { i32 }),
            SchemaType::S64 { .. } => Ok(quote! { i64 }),
            SchemaType::U8 { .. } => Ok(quote! { u8 }),
            SchemaType::U16 { .. } => Ok(quote! { u16 }),
            SchemaType::U32 { .. } => Ok(quote! { u32 }),
            SchemaType::U64 { .. } => Ok(quote! { u64 }),
            SchemaType::F32 { .. } => Ok(quote! { f32 }),
            SchemaType::F64 { .. } => Ok(quote! { f64 }),
            SchemaType::Char { .. } => Ok(quote! { char }),
            SchemaType::String { .. } => Ok(quote! { String }),
            SchemaType::Option { inner, .. } => {
                let inner = self.type_reference(inner, box_recursive)?;
                Ok(quote! { Option<#inner> })
            }
            SchemaType::List { element, .. } => {
                let inner = self.type_reference(element, false)?;
                Ok(quote! { Vec<#inner> })
            }
            SchemaType::FixedList { element, .. } => {
                let inner = self.type_reference(element, false)?;
                Ok(quote! { Vec<#inner> })
            }
            SchemaType::Map { key, value, .. } => {
                let k = self.type_reference(key, false)?;
                let v = self.type_reference(value, false)?;
                Ok(quote! { Vec<(#k, #v)> })
            }
            SchemaType::Tuple { elements, .. } => {
                let mut items = Vec::new();
                for item in elements {
                    items.push(self.type_reference(item, box_recursive)?);
                }
                if items.len() == 1 {
                    Ok(quote! { ( #(#items),* , ) })
                } else {
                    Ok(quote! { ( #(#items),* ) })
                }
            }
            SchemaType::Result { spec, .. } => {
                let ok = match spec.ok.as_deref() {
                    Some(t) => self.type_reference(t, box_recursive)?,
                    None => quote! { () },
                };
                let err = match spec.err.as_deref() {
                    Some(t) => self.type_reference(t, box_recursive)?,
                    None => quote! { () },
                };
                Ok(quote! { Result<#ok, #err> })
            }
            SchemaType::Text { .. } | SchemaType::Binary { .. } => Err(anyhow!(
                "Bare text/binary rich scalars have no Rust bridge type; \
                 wrap them in the unstructured text/binary variant ({typ:?})"
            )),
            SchemaType::Path { .. } => Ok(quote! { String }),
            SchemaType::Url { .. } => Ok(quote! { String }),
            SchemaType::Datetime { .. } => Ok(quote! { String }),
            SchemaType::Duration { .. } => Ok(quote! { i64 }),
            SchemaType::Ref { .. }
            | SchemaType::Variant { .. }
            | SchemaType::Enum { .. }
            | SchemaType::Flags { .. }
            | SchemaType::Record { .. }
            | SchemaType::Union { .. } => Err(anyhow!("Missing type name for {typ:?}")),
            SchemaType::Quantity { .. }
            | SchemaType::Secret { .. }
            | SchemaType::QuotaToken { .. }
            | SchemaType::Future { .. }
            | SchemaType::Stream { .. } => Err(anyhow!(
                "Cannot emit Rust type reference for unsupported schema variant: {typ:?}"
            )),
        }
    }

    // --- Text / binary restriction enums ------------------------------------

    fn unstructured_text_type(&mut self, restrictions: &TextRestrictions) -> TokenStream {
        match &restrictions.languages {
            Some(langs) if !langs.is_empty() => {
                let enum_ty = self.get_languages_enum(langs);
                quote! { crate::__golem_bridge_runtime::agentic::UnstructuredText<#enum_ty> }
            }
            _ => quote! { crate::__golem_bridge_runtime::agentic::UnstructuredText },
        }
    }

    fn unstructured_binary_type(&mut self, restrictions: &BinaryRestrictions) -> TokenStream {
        match &restrictions.mime_types {
            Some(mimes) if !mimes.is_empty() => {
                let enum_ty = self.get_mimetypes_enum(mimes);
                quote! { crate::__golem_bridge_runtime::agentic::UnstructuredBinary<#enum_ty> }
            }
            _ if self.mode == RustBridgeMode::GuestWasmRpc => {
                quote! { crate::__golem_bridge_runtime::agentic::UnstructuredBinary<String> }
            }
            _ => quote! { crate::__golem_bridge_runtime::agentic::UnstructuredBinary },
        }
    }

    fn get_languages_enum(&mut self, languages: &[String]) -> TokenStream {
        let languages = languages.to_vec();
        let name = if let Some((_, name)) = self
            .generated_language_enums
            .iter()
            .find(|(l, _)| *l == languages)
        {
            name.clone()
        } else {
            let name = format!("Languages{}", self.generated_language_enums.len());
            self.generated_language_enums
                .push((languages, name.clone()));
            name
        };
        let ident = Ident::new(&name, Span::call_site());
        quote! { crate::languages::#ident }
    }

    fn get_mimetypes_enum(&mut self, mime_types: &[String]) -> TokenStream {
        let mime_types = mime_types.to_vec();
        let name = if let Some((_, name)) = self
            .generated_mimetypes_enums
            .iter()
            .find(|(m, _)| *m == mime_types)
        {
            name.clone()
        } else {
            let name = format!("Mimetypes{}", self.generated_mimetypes_enums.len());
            self.generated_mimetypes_enums
                .push((mime_types, name.clone()));
            name
        };
        let ident = Ident::new(&name, Span::call_site());
        quote! { crate::mimetypes::#ident }
    }

    fn languages_module(&self) -> TokenStream {
        if self.generated_language_enums.is_empty() {
            return quote! {};
        }
        let mut enums = Vec::new();
        for (codes, name) in &self.generated_language_enums {
            let ident = Ident::new(name, Span::call_site());
            let mut cases = Vec::new();
            let mut code_strings = Vec::new();
            let mut from_cases = Vec::new();
            let mut to_cases = Vec::new();
            for code in codes {
                let case_ident = Ident::new(
                    &to_rust_ident(code, false).to_upper_camel_case(),
                    Span::call_site(),
                );
                cases.push(quote! { #case_ident });
                code_strings.push(quote! { #code });
                from_cases.push(quote! { #code => Some(Self::#case_ident) });
                to_cases.push(quote! { Self::#case_ident => #code.to_string() });
            }
            enums.push(quote! {
                #[derive(Debug, Clone)]
                pub enum #ident {
                    #(#cases),*
                }

                impl crate::__golem_bridge_runtime::agentic::AllowedLanguages for #ident {
                    fn all() -> &'static [&'static str] {
                        &[#(#code_strings),*]
                    }

                    fn from_language_code(code: &str) -> Option<Self> {
                        match code {
                            #(#from_cases,)*
                            _ => None,
                        }
                    }

                    fn to_language_code(&self) -> String {
                        match self {
                            #(#to_cases),*
                        }
                    }
                }
            });
        }
        quote! {
            pub mod languages {
                #(#enums)*
            }
        }
    }

    fn mimetypes_module(&self) -> TokenStream {
        if self.generated_mimetypes_enums.is_empty() {
            return quote! {};
        }
        let mut enums = Vec::new();
        for (mimes, name) in &self.generated_mimetypes_enums {
            let ident = Ident::new(name, Span::call_site());
            let mime_type_arg = Ident::new("mime_type", Span::call_site());
            let from_method = self.mime_type_from_method(&mime_type_arg);
            let to_method = self.mime_type_to_method();
            let mut cases = Vec::new();
            let mut code_strings = Vec::new();
            let mut from_cases = Vec::new();
            let mut to_cases = Vec::new();
            for mime in mimes {
                let case_ident = Ident::new(
                    &to_rust_ident(mime, false).to_upper_camel_case(),
                    Span::call_site(),
                );
                cases.push(quote! { #case_ident });
                code_strings.push(quote! { #mime });
                from_cases.push(quote! { #mime => Some(Self::#case_ident) });
                to_cases.push(quote! { Self::#case_ident => #mime.to_string() });
            }
            enums.push(quote! {
                #[derive(Debug, Clone)]
                pub enum #ident {
                    #(#cases),*
                }

                impl crate::__golem_bridge_runtime::agentic::AllowedMimeTypes for #ident {
                    fn all() -> &'static [&'static str] {
                        &[#(#code_strings),*]
                    }

                    #from_method {
                        match #mime_type_arg {
                            #(#from_cases,)*
                            _ => None,
                        }
                    }

                    #to_method {
                        match self {
                            #(#to_cases),*
                        }
                    }
                }
            });
        }
        quote! {
            pub mod mimetypes {
                #(#enums)*
            }
        }
    }

    fn mime_type_from_method(&self, arg: &Ident) -> TokenStream {
        match self.mode {
            RustBridgeMode::ExternalRest => {
                quote! { fn from_mime_type(#arg: &str) -> Option<Self> }
            }
            RustBridgeMode::GuestWasmRpc => quote! { fn from_string(#arg: &str) -> Option<Self> },
        }
    }

    fn mime_type_to_method(&self) -> TokenStream {
        match self.mode {
            RustBridgeMode::ExternalRest => quote! { fn to_mime_type(&self) -> String },
            RustBridgeMode::GuestWasmRpc => quote! { fn to_string(&self) -> String },
        }
    }

    // --- Identifier helpers -------------------------------------------------

    fn to_rust_ident(&self, name: &str) -> String {
        to_rust_ident(name, self.same_language)
    }

    fn to_rust_case_name(&self, name: &str) -> String {
        if self.same_language && is_valid_rust_ident(name) {
            to_rust_ident(name, true)
        } else {
            to_rust_ident(name, false).to_upper_camel_case()
        }
    }

    fn package_crate_name(&self) -> String {
        Self::package_crate_name_for_mode(&self.agent_type.type_name, self.mode)
    }

    fn package_crate_name_for_mode(
        agent_type_name: &golem_common::model::agent::AgentTypeName,
        mode: RustBridgeMode,
    ) -> String {
        match mode {
            RustBridgeMode::ExternalRest => bridge_client_directory_name(agent_type_name),
            RustBridgeMode::GuestWasmRpc => {
                bridge_client_directory_name_for_mode(agent_type_name, BridgeMode::Guest)
            }
        }
    }

    fn guest_client_struct_name_string(agent_type_name: &str) -> String {
        if agent_type_name == "__golem_bridge_runtime" {
            "__GolemBridgeRuntimeClient".to_string()
        } else {
            agent_type_name.to_string()
        }
    }

    fn root_type_name_conflicts(&self, name: &str) -> bool {
        self.agent_type.type_name.0 == name
            || self.type_naming.types().any(|(_, type_name)| {
                matches!(type_name, RustTypeName::Derived(type_name) if type_name == name)
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::model::Empty;
    use golem_common::model::agent::{AgentMode, AgentTypeName, Snapshotting};
    use golem_common::schema::metadata::TypeId;
    use golem_common::schema::{
        AgentConstructorSchema, MetadataEnvelope, NamedField, NamedFieldType, SchemaGraph,
        SchemaType,
    };
    use test_r::test;

    #[test]
    fn package_crate_name_is_mode_separated() {
        let agent_type_name = AgentTypeName("AlphaAgent".to_string());

        assert_eq!(
            RustBridgeGenerator::package_crate_name_for_mode(
                &agent_type_name,
                RustBridgeMode::ExternalRest
            ),
            "alpha-agent-client"
        );
        assert_eq!(
            RustBridgeGenerator::package_crate_name_for_mode(
                &agent_type_name,
                RustBridgeMode::GuestWasmRpc
            ),
            "alpha-agent-guest-client"
        );
    }

    #[test]
    fn external_runtime_prelude_preserves_rest_runtime_paths() {
        let mut generator =
            RustBridgeGenerator::new(minimal_agent_type("AlphaAgent"), Utf8Path::new("."), true)
                .unwrap();

        let rendered = generator.generate_lib_rs_tokens().unwrap().to_string();

        assert!(rendered.contains("pub use golem_client :: bridge :: ClientError"));
        assert!(rendered.contains("pub mod __golem_bridge_runtime"));
        assert!(rendered.contains("pub use golem_client :: bridge :: GolemServer"));
        assert!(rendered.contains("pub use golem_common :: schema :: *"));
        assert!(rendered.contains("pub use golem_common :: agentic :: *"));
        assert!(rendered.contains("golem_client :: api :: AgentClient :: create_agent"));
        assert!(rendered.contains("reqwest_middleware :: ClientWithMiddleware"));
    }

    #[test]
    fn external_generation_preserves_rest_cargo_dependencies_and_api_shape() {
        let dir = tempfile::TempDir::new().unwrap();
        let target_path =
            Utf8PathBuf::from_path_buf(dir.path().join("alpha-agent-client")).unwrap();
        let mut agent_type = minimal_agent_type("AlphaAgent");
        agent_type.mode = AgentMode::Durable;
        let mut generator = RustBridgeGenerator::new(agent_type, &target_path, true).unwrap();

        generator.generate().unwrap();

        let cargo_toml = std::fs::read_to_string(target_path.join("Cargo.toml")).unwrap();
        assert!(cargo_toml.contains("name = \"alpha-agent-client\""));
        for dependency in [
            "chrono",
            "golem-client",
            "golem-common",
            "reqwest",
            "reqwest-middleware",
            "serde_json",
            "uuid",
        ] {
            assert!(
                cargo_toml.contains(&format!("{dependency} = ")),
                "missing external REST dependency {dependency}:\n{cargo_toml}"
            );
        }
        assert!(!cargo_toml.contains("golem-rust"));

        let lib_rs = std::fs::read_to_string(target_path.join("src/lib.rs")).unwrap();
        for rest_shape in [
            "pub async fn get(",
            "pub async fn get_phantom(",
            "pub async fn new_phantom(",
            "pub fn configure(",
            "server: golem_client::bridge::GolemServer",
            "pub mod __golem_bridge_runtime",
            "pub use golem_client::bridge::GolemServer",
            "pub use golem_client::bridge::ClientError",
            "pub use golem_common::schema::*",
            "pub use golem_common::agentic::*",
            "golem_client::api::AgentClient::create_agent",
            "golem_client::api::AgentClient::invoke_agent",
            "reqwest_middleware::ClientWithMiddleware",
        ] {
            assert!(
                lib_rs.contains(rest_shape),
                "missing external REST API shape {rest_shape}:\n{lib_rs}"
            );
        }
        assert!(!lib_rs.contains("golem_rust"));
    }

    #[test]
    fn guest_runtime_prelude_uses_golem_rust_paths() {
        let rendered = RustRuntimeConfig::new(RustBridgeMode::GuestWasmRpc)
            .generated_prelude()
            .to_string();

        assert!(rendered.contains("pub enum ClientError"));
        assert!(rendered.contains("pub use golem_rust :: SchemaValue"));
        assert!(rendered.contains("pub use golem_rust :: schema"));
        assert!(rendered.contains("pub use golem_rust :: agentic :: *"));
        assert!(!rendered.contains("golem_common"));
        assert!(!rendered.contains("golem_client"));
    }

    #[test]
    fn guest_runtime_prelude_compiles_with_generated_golem_rust_dependency_flags() {
        let dir = tempfile::TempDir::new().unwrap();
        let golem_rust_path = workspace_root().unwrap().join("sdks/rust/golem-rust");
        let golem_rust_path = golem_rust_path.to_string_lossy();

        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(
            dir.path().join("Cargo.toml"),
            format!(
                r#"[package]
name = "guest-runtime-prelude-generated-flags-check"
version = "0.0.1"
edition = "2021"

[workspace]

[dependencies]
golem-rust = {{ path = {golem_rust_path:?}, default-features = false, features = ["export_golem_agentic", "macro"] }}
"#
            ),
        )
        .unwrap();

        let prelude = prettyplease::unparse(
            &syn::parse2::<syn::File>(
                RustRuntimeConfig::new(RustBridgeMode::GuestWasmRpc).generated_prelude(),
            )
            .unwrap(),
        );
        let body = guest_generated_unstructured_body();
        std::fs::write(dir.path().join("src/lib.rs"), format!("{prelude}\n{body}")).unwrap();

        let output = std::process::Command::new("cargo")
            .arg("check")
            .current_dir(dir.path())
            .output()
            .unwrap();

        assert!(
            output.status.success(),
            "cargo check failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn guest_generated_unstructured_body() -> &'static str {
        r#"
pub fn takes_unrestricted_binary(
    _value: crate::__golem_bridge_runtime::agentic::UnstructuredBinary<String>,
) {
}

pub mod mimetypes {
    #[derive(Debug, Clone)]
    pub enum Mimetypes0 {
        ApplicationJson,
    }

    impl crate::__golem_bridge_runtime::agentic::AllowedMimeTypes for Mimetypes0 {
        fn all() -> &'static [&'static str] {
            &["application/json"]
        }

        fn from_string(mime_type: &str) -> Option<Self> {
            match mime_type {
                "application/json" => Some(Self::ApplicationJson),
                _ => None,
            }
        }

        fn to_string(&self) -> String {
            match self {
                Self::ApplicationJson => "application/json".to_string(),
            }
        }
    }
}

pub fn encode_text(
    value: crate::__golem_bridge_runtime::agentic::UnstructuredText,
) -> Result<crate::__golem_bridge_runtime::schema::SchemaValue, String> {
    <crate::__golem_bridge_runtime::agentic::UnstructuredText as crate::__golem_bridge_runtime::agentic::Schema>::to_schema_value(value)
}

pub fn decode_text(
    value: crate::__golem_bridge_runtime::schema::SchemaValue,
) -> Result<crate::__golem_bridge_runtime::agentic::UnstructuredText, String> {
    <crate::__golem_bridge_runtime::agentic::UnstructuredText as crate::__golem_bridge_runtime::agentic::Schema>::from_schema_value(
        value,
        <crate::__golem_bridge_runtime::agentic::UnstructuredText as crate::__golem_bridge_runtime::agentic::Schema>::get_type(),
    )
}
"#
    }

    #[test]
    fn guest_generation_emits_wasm_rpc_cargo_dependencies_and_api_shape() {
        let dir = tempfile::TempDir::new().unwrap();
        let target_path =
            Utf8PathBuf::from_path_buf(dir.path().join("alpha-agent-guest-client")).unwrap();
        let mut agent_type = minimal_agent_type("AlphaAgent");
        agent_type.mode = AgentMode::Durable;
        agent_type.methods.push(AgentMethodSchema {
            name: "run".to_string(),
            description: String::new(),
            prompt_hint: None,
            input_schema: InputSchema::parameters(vec![NamedField::user_supplied(
                "value",
                SchemaType::s32(),
            )]),
            output_schema: OutputSchema::Single(Box::new(SchemaType::string())),
            http_endpoint: vec![],
            read_only: None,
        });
        let mut generator = RustBridgeGenerator::new_with_mode(
            agent_type,
            &target_path,
            true,
            RustBridgeMode::GuestWasmRpc,
        )
        .unwrap();

        generator.generate().unwrap();

        let cargo_toml = std::fs::read_to_string(target_path.join("Cargo.toml")).unwrap();
        assert!(cargo_toml.contains("name = \"alpha-agent-guest-client\""));
        assert!(cargo_toml.contains("golem-rust"));
        assert!(cargo_toml.contains("export_golem_agentic"));
        assert!(!cargo_toml.contains("golem-client"));
        assert!(!cargo_toml.contains("reqwest"));

        let lib_rs = std::fs::read_to_string(target_path.join("src/lib.rs")).unwrap();
        for guest_shape in [
            "agent_id: String",
            "phantom_id: Option<golem_rust::Uuid>",
            "wasm_rpc: golem_rust::golem_agentic::golem::agent::host::WasmRpc",
            "pub fn get(",
            "pub fn get_phantom(",
            "pub fn new_phantom(",
            "golem_rust::golem_agentic::golem::agent::host::make_agent_id",
            "golem_rust::golem_agentic::golem::agent::host::WasmRpc::new",
            "async_invoke_and_await",
            "await_invoke_schema_value_result",
            ".invoke(",
            "schedule_invocation",
            "MissingResult",
        ] {
            assert!(
                lib_rs.contains(guest_shape),
                "missing guest wasm-rpc API shape {guest_shape}:\n{lib_rs}"
            );
        }
        assert!(!lib_rs.contains("golem_client"));
        assert!(!lib_rs.contains("reqwest"));
        assert!(!lib_rs.contains("constructor_parameters"));

        let output = std::process::Command::new("cargo")
            .arg("check")
            .current_dir(&target_path)
            .output()
            .unwrap();

        assert!(
            output.status.success(),
            "generated guest crate cargo check failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn guest_generation_compiles_with_non_copy_constructor_parameters() {
        let dir = tempfile::TempDir::new().unwrap();
        let target_path =
            Utf8PathBuf::from_path_buf(dir.path().join("alpha-agent-guest-client")).unwrap();
        let mut agent_type = minimal_agent_type("AlphaAgent");
        agent_type.mode = AgentMode::Durable;
        agent_type.constructor.input_schema =
            InputSchema::parameters(vec![NamedField::user_supplied(
                "name",
                SchemaType::string(),
            )]);
        let mut generator = RustBridgeGenerator::new_with_mode(
            agent_type,
            &target_path,
            true,
            RustBridgeMode::GuestWasmRpc,
        )
        .unwrap();

        generator.generate().unwrap();

        let output = std::process::Command::new("cargo")
            .arg("check")
            .current_dir(&target_path)
            .output()
            .unwrap();

        assert!(
            output.status.success(),
            "generated guest crate with non-Copy constructor parameter failed cargo check\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn guest_generation_compiles_when_constructor_parameter_matches_internal_name() {
        let dir = tempfile::TempDir::new().unwrap();
        let target_path =
            Utf8PathBuf::from_path_buf(dir.path().join("alpha-agent-guest-client")).unwrap();
        let mut agent_type = minimal_agent_type("AlphaAgent");
        agent_type.mode = AgentMode::Durable;
        agent_type.constructor.input_schema =
            InputSchema::parameters(vec![NamedField::user_supplied(
                "phantom_id",
                SchemaType::s32(),
            )]);
        let mut generator = RustBridgeGenerator::new_with_mode(
            agent_type,
            &target_path,
            true,
            RustBridgeMode::GuestWasmRpc,
        )
        .unwrap();

        generator.generate().unwrap();

        let output = std::process::Command::new("cargo")
            .arg("check")
            .current_dir(&target_path)
            .output()
            .unwrap();

        assert!(
            output.status.success(),
            "generated guest crate with constructor parameter matching an internal name failed cargo check\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn guest_generation_compiles_when_agent_type_name_matches_guest_client_error() {
        let dir = tempfile::TempDir::new().unwrap();
        let target_path =
            Utf8PathBuf::from_path_buf(dir.path().join("client-error-guest-client")).unwrap();
        let mut agent_type = minimal_agent_type("ClientError");
        agent_type.mode = AgentMode::Durable;
        let mut generator = RustBridgeGenerator::new_with_mode(
            agent_type,
            &target_path,
            true,
            RustBridgeMode::GuestWasmRpc,
        )
        .unwrap();

        generator.generate().unwrap();

        let output = std::process::Command::new("cargo")
            .arg("check")
            .current_dir(&target_path)
            .output()
            .unwrap();

        assert!(
            output.status.success(),
            "generated guest crate with agent type named ClientError failed cargo check\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn guest_generation_compiles_when_agent_type_name_matches_guest_runtime_module() {
        let dir = tempfile::TempDir::new().unwrap();
        let target_path =
            Utf8PathBuf::from_path_buf(dir.path().join("runtime-module-guest-client")).unwrap();
        let mut agent_type = minimal_agent_type("__golem_bridge_runtime");
        agent_type.source_language = "rust".to_string();
        agent_type.mode = AgentMode::Durable;
        let mut generator = RustBridgeGenerator::new_with_mode(
            agent_type,
            &target_path,
            true,
            RustBridgeMode::GuestWasmRpc,
        )
        .unwrap();

        generator.generate().unwrap();

        let output = std::process::Command::new("cargo")
            .arg("check")
            .current_dir(&target_path)
            .output()
            .unwrap();

        assert!(
            output.status.success(),
            "generated guest crate with agent type named __golem_bridge_runtime failed cargo check\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn guest_generation_compiles_when_method_parameter_is_named_when() {
        let dir = tempfile::TempDir::new().unwrap();
        let target_path =
            Utf8PathBuf::from_path_buf(dir.path().join("alpha-agent-guest-client")).unwrap();
        let mut agent_type = minimal_agent_type("AlphaAgent");
        agent_type.methods.push(AgentMethodSchema {
            name: "run".to_string(),
            description: String::new(),
            prompt_hint: None,
            input_schema: InputSchema::parameters(vec![NamedField::user_supplied(
                "when",
                SchemaType::s32(),
            )]),
            output_schema: OutputSchema::Unit,
            http_endpoint: vec![],
            read_only: None,
        });
        let mut generator = RustBridgeGenerator::new_with_mode(
            agent_type,
            &target_path,
            true,
            RustBridgeMode::GuestWasmRpc,
        )
        .unwrap();

        generator.generate().unwrap();

        let output = std::process::Command::new("cargo")
            .arg("check")
            .current_dir(&target_path)
            .output()
            .unwrap();

        assert!(
            output.status.success(),
            "generated guest crate with method parameter named when failed cargo check\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn guest_generation_compiles_when_method_name_matches_phantom_id_accessor() {
        let dir = tempfile::TempDir::new().unwrap();
        let target_path =
            Utf8PathBuf::from_path_buf(dir.path().join("alpha-agent-guest-client")).unwrap();
        let mut agent_type = minimal_agent_type("AlphaAgent");
        agent_type.methods.push(AgentMethodSchema {
            name: "phantom_id".to_string(),
            description: String::new(),
            prompt_hint: None,
            input_schema: InputSchema::parameters(vec![]),
            output_schema: OutputSchema::Unit,
            http_endpoint: vec![],
            read_only: None,
        });
        let mut generator = RustBridgeGenerator::new_with_mode(
            agent_type,
            &target_path,
            true,
            RustBridgeMode::GuestWasmRpc,
        )
        .unwrap();

        generator.generate().unwrap();

        let output = std::process::Command::new("cargo")
            .arg("check")
            .current_dir(&target_path)
            .output()
            .unwrap();

        assert!(
            output.status.success(),
            "generated guest crate with method named phantom_id failed cargo check\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn guest_generation_compiles_when_method_name_matches_agent_id_accessor() {
        let dir = tempfile::TempDir::new().unwrap();
        let target_path =
            Utf8PathBuf::from_path_buf(dir.path().join("alpha-agent-guest-client")).unwrap();
        let mut agent_type = minimal_agent_type("AlphaAgent");
        agent_type.methods.push(AgentMethodSchema {
            name: "agent_id".to_string(),
            description: String::new(),
            prompt_hint: None,
            input_schema: InputSchema::parameters(vec![]),
            output_schema: OutputSchema::Unit,
            http_endpoint: vec![],
            read_only: None,
        });
        let mut generator = RustBridgeGenerator::new_with_mode(
            agent_type,
            &target_path,
            true,
            RustBridgeMode::GuestWasmRpc,
        )
        .unwrap();

        generator.generate().unwrap();

        let output = std::process::Command::new("cargo")
            .arg("check")
            .current_dir(&target_path)
            .output()
            .unwrap();

        assert!(
            output.status.success(),
            "generated guest crate with method named agent_id failed cargo check\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn guest_generation_compiles_when_method_name_matches_guest_get_helper() {
        let dir = tempfile::TempDir::new().unwrap();
        let target_path =
            Utf8PathBuf::from_path_buf(dir.path().join("alpha-agent-guest-client")).unwrap();
        let mut agent_type = minimal_agent_type("AlphaAgent");
        agent_type.mode = AgentMode::Durable;
        agent_type.methods.push(AgentMethodSchema {
            name: "get".to_string(),
            description: String::new(),
            prompt_hint: None,
            input_schema: InputSchema::parameters(vec![]),
            output_schema: OutputSchema::Unit,
            http_endpoint: vec![],
            read_only: None,
        });
        let mut generator = RustBridgeGenerator::new_with_mode(
            agent_type,
            &target_path,
            true,
            RustBridgeMode::GuestWasmRpc,
        )
        .unwrap();

        generator.generate().unwrap();

        let output = std::process::Command::new("cargo")
            .arg("check")
            .current_dir(&target_path)
            .output()
            .unwrap();

        assert!(
            output.status.success(),
            "generated guest crate with method named get failed cargo check\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn guest_generation_compiles_when_get_helper_deconflicted_name_matches_user_method() {
        let dir = tempfile::TempDir::new().unwrap();
        let target_path =
            Utf8PathBuf::from_path_buf(dir.path().join("alpha-agent-guest-client")).unwrap();
        let mut agent_type = minimal_agent_type("AlphaAgent");
        agent_type.mode = AgentMode::Durable;
        for method_name in ["get", "get_1"] {
            agent_type.methods.push(AgentMethodSchema {
                name: method_name.to_string(),
                description: String::new(),
                prompt_hint: None,
                input_schema: InputSchema::parameters(vec![]),
                output_schema: OutputSchema::Unit,
                http_endpoint: vec![],
                read_only: None,
            });
        }
        let mut generator = RustBridgeGenerator::new_with_mode(
            agent_type,
            &target_path,
            true,
            RustBridgeMode::GuestWasmRpc,
        )
        .unwrap();

        generator.generate().unwrap();

        let output = std::process::Command::new("cargo")
            .arg("check")
            .current_dir(&target_path)
            .output()
            .unwrap();

        assert!(
            output.status.success(),
            "generated guest crate with methods named get and get_1 failed cargo check\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn guest_generation_compiles_when_method_name_matches_trigger_wrapper_for_another_method() {
        let dir = tempfile::TempDir::new().unwrap();
        let target_path =
            Utf8PathBuf::from_path_buf(dir.path().join("alpha-agent-guest-client")).unwrap();
        let mut agent_type = minimal_agent_type("AlphaAgent");
        for method_name in ["run", "trigger_run"] {
            agent_type.methods.push(AgentMethodSchema {
                name: method_name.to_string(),
                description: String::new(),
                prompt_hint: None,
                input_schema: InputSchema::parameters(vec![]),
                output_schema: OutputSchema::Unit,
                http_endpoint: vec![],
                read_only: None,
            });
        }
        let mut generator = RustBridgeGenerator::new_with_mode(
            agent_type,
            &target_path,
            true,
            RustBridgeMode::GuestWasmRpc,
        )
        .unwrap();

        generator.generate().unwrap();

        let output = std::process::Command::new("cargo")
            .arg("check")
            .current_dir(&target_path)
            .output()
            .unwrap();

        assert!(
            output.status.success(),
            "generated guest crate with method named trigger_run next to run failed cargo check\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn guest_generation_compiles_when_reserved_trigger_wrapper_suffix_matches_another_wrapper() {
        let dir = tempfile::TempDir::new().unwrap();
        let target_path =
            Utf8PathBuf::from_path_buf(dir.path().join("alpha-agent-guest-client")).unwrap();
        let mut agent_type = minimal_agent_type("AlphaAgent");
        agent_type.source_language = "rust".to_string();
        for method_name in ["run", "__golem_bridge_trigger_run", "run_1"] {
            agent_type.methods.push(AgentMethodSchema {
                name: method_name.to_string(),
                description: String::new(),
                prompt_hint: None,
                input_schema: InputSchema::parameters(vec![]),
                output_schema: OutputSchema::Unit,
                http_endpoint: vec![],
                read_only: None,
            });
        }
        let mut generator = RustBridgeGenerator::new_with_mode(
            agent_type,
            &target_path,
            true,
            RustBridgeMode::GuestWasmRpc,
        )
        .unwrap();

        generator.generate().unwrap();

        let output = std::process::Command::new("cargo")
            .arg("check")
            .current_dir(&target_path)
            .output()
            .unwrap();

        assert!(
            output.status.success(),
            "generated guest crate with a reserved trigger wrapper suffix matching another wrapper failed cargo check\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn guest_generation_compiles_when_rust_method_name_matches_reserved_trigger_wrapper() {
        let dir = tempfile::TempDir::new().unwrap();
        let target_path =
            Utf8PathBuf::from_path_buf(dir.path().join("alpha-agent-guest-client")).unwrap();
        let mut agent_type = minimal_agent_type("AlphaAgent");
        agent_type.source_language = "rust".to_string();
        for method_name in ["run", "__golem_bridge_trigger_run"] {
            agent_type.methods.push(AgentMethodSchema {
                name: method_name.to_string(),
                description: String::new(),
                prompt_hint: None,
                input_schema: InputSchema::parameters(vec![]),
                output_schema: OutputSchema::Unit,
                http_endpoint: vec![],
                read_only: None,
            });
        }
        let mut generator = RustBridgeGenerator::new_with_mode(
            agent_type,
            &target_path,
            true,
            RustBridgeMode::GuestWasmRpc,
        )
        .unwrap();

        generator.generate().unwrap();

        let output = std::process::Command::new("cargo")
            .arg("check")
            .current_dir(&target_path)
            .output()
            .unwrap();

        assert!(
            output.status.success(),
            "generated guest crate with Rust method named __golem_bridge_trigger_run next to run failed cargo check\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn guest_generation_compiles_when_constructor_parameter_matches_config_parameter_name() {
        let dir = tempfile::TempDir::new().unwrap();
        let target_path =
            Utf8PathBuf::from_path_buf(dir.path().join("alpha-agent-guest-client")).unwrap();
        let mut agent_type = minimal_agent_type("AlphaAgent");
        agent_type.mode = AgentMode::Durable;
        agent_type.constructor.input_schema =
            InputSchema::parameters(vec![NamedField::user_supplied(
                "config_foo",
                SchemaType::s32(),
            )]);
        agent_type.config.push(AgentConfigDeclarationSchema {
            source: AgentConfigSource::Local,
            path: vec!["foo".to_string()],
            value_type: SchemaType::string(),
        });
        let mut generator = RustBridgeGenerator::new_with_mode(
            agent_type,
            &target_path,
            true,
            RustBridgeMode::GuestWasmRpc,
        )
        .unwrap();

        generator.generate().unwrap();

        let output = std::process::Command::new("cargo")
            .arg("check")
            .current_dir(&target_path)
            .output()
            .unwrap();

        assert!(
            output.status.success(),
            "generated guest crate with constructor parameter matching local config parameter failed cargo check\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn guest_generation_emits_self_contained_typed_config_schema_values() {
        let dir = tempfile::TempDir::new().unwrap();
        let target_path =
            Utf8PathBuf::from_path_buf(dir.path().join("alpha-agent-guest-client")).unwrap();
        let mut agent_type = minimal_agent_type("AlphaAgent");
        agent_type.mode = AgentMode::Durable;
        let config_type_id = TypeId::new("config-shared");
        agent_type.schema = SchemaGraph {
            defs: vec![SchemaTypeDef {
                id: config_type_id.clone(),
                name: Some("ConfigShared".to_string()),
                body: SchemaType::record(vec![NamedFieldType {
                    name: "label".to_string(),
                    body: SchemaType::string(),
                    metadata: MetadataEnvelope::default(),
                }]),
            }],
            root: SchemaType::record(vec![]),
        };
        agent_type.config.push(AgentConfigDeclarationSchema {
            source: AgentConfigSource::Local,
            path: vec!["shared".to_string()],
            value_type: SchemaType::ref_to(config_type_id),
        });
        let mut generator = RustBridgeGenerator::new_with_mode(
            agent_type,
            &target_path,
            true,
            RustBridgeMode::GuestWasmRpc,
        )
        .unwrap();

        generator.generate().unwrap();

        let lib_rs = std::fs::read_to_string(target_path.join("src/lib.rs")).unwrap();
        assert!(
            lib_rs.contains("config-shared"),
            "generated typed config schema graph must include referenced definitions:\n{lib_rs}"
        );
        assert!(
            lib_rs.contains("TypedSchemaValue::new"),
            "generated typed config encoding must build a typed value:\n{lib_rs}"
        );
        assert!(
            lib_rs.contains("golem_rust::encode_typed_schema_value"),
            "generated typed config encoding must use guest golem-rust wire encoding:\n{lib_rs}"
        );

        let output = std::process::Command::new("cargo")
            .arg("check")
            .current_dir(&target_path)
            .output()
            .unwrap();

        assert!(
            output.status.success(),
            "generated guest crate with referenced local config type failed cargo check\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn guest_generation_compiles_when_constructor_parameter_is_named_agent_config() {
        let dir = tempfile::TempDir::new().unwrap();
        let target_path =
            Utf8PathBuf::from_path_buf(dir.path().join("alpha-agent-guest-client")).unwrap();
        let mut agent_type = minimal_agent_type("AlphaAgent");
        agent_type.mode = AgentMode::Durable;
        agent_type.constructor.input_schema =
            InputSchema::parameters(vec![NamedField::user_supplied(
                "agent_config",
                SchemaType::s32(),
            )]);
        agent_type.config.push(AgentConfigDeclarationSchema {
            source: AgentConfigSource::Local,
            path: vec!["foo".to_string()],
            value_type: SchemaType::string(),
        });
        let mut generator = RustBridgeGenerator::new_with_mode(
            agent_type,
            &target_path,
            true,
            RustBridgeMode::GuestWasmRpc,
        )
        .unwrap();

        generator.generate().unwrap();

        let output = std::process::Command::new("cargo")
            .arg("check")
            .current_dir(&target_path)
            .output()
            .unwrap();

        assert!(
            output.status.success(),
            "generated guest crate with constructor parameter named agent_config failed cargo check\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn guest_generation_compiles_when_rust_constructor_parameter_matches_reserved_phantom_id_name()
    {
        let dir = tempfile::TempDir::new().unwrap();
        let target_path =
            Utf8PathBuf::from_path_buf(dir.path().join("alpha-agent-guest-client")).unwrap();
        let mut agent_type = minimal_agent_type("AlphaAgent");
        agent_type.source_language = "rust".to_string();
        agent_type.mode = AgentMode::Durable;
        agent_type.constructor.input_schema =
            InputSchema::parameters(vec![NamedField::user_supplied(
                "__golem_bridge_phantom_id",
                SchemaType::s32(),
            )]);
        let mut generator = RustBridgeGenerator::new_with_mode(
            agent_type,
            &target_path,
            true,
            RustBridgeMode::GuestWasmRpc,
        )
        .unwrap();

        generator.generate().unwrap();

        let output = std::process::Command::new("cargo")
            .arg("check")
            .current_dir(&target_path)
            .output()
            .unwrap();

        assert!(
            output.status.success(),
            "generated guest crate with Rust constructor parameter matching reserved phantom id name failed cargo check\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn guest_generation_compiles_when_method_parameter_names_sanitize_to_same_ident() {
        let dir = tempfile::TempDir::new().unwrap();
        let target_path =
            Utf8PathBuf::from_path_buf(dir.path().join("alpha-agent-guest-client")).unwrap();
        let mut agent_type = minimal_agent_type("AlphaAgent");
        agent_type.methods.push(AgentMethodSchema {
            name: "run".to_string(),
            description: String::new(),
            prompt_hint: None,
            input_schema: InputSchema::parameters(vec![
                NamedField::user_supplied("foo-bar", SchemaType::s32()),
                NamedField::user_supplied("foo_bar", SchemaType::s32()),
            ]),
            output_schema: OutputSchema::Unit,
            http_endpoint: vec![],
            read_only: None,
        });
        let mut generator = RustBridgeGenerator::new_with_mode(
            agent_type,
            &target_path,
            true,
            RustBridgeMode::GuestWasmRpc,
        )
        .unwrap();

        generator.generate().unwrap();

        let output = std::process::Command::new("cargo")
            .arg("check")
            .current_dir(&target_path)
            .output()
            .unwrap();

        assert!(
            output.status.success(),
            "generated guest crate with method parameters that sanitize to the same Rust ident failed cargo check\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn minimal_agent_type(type_name: &str) -> AgentTypeSchema {
        AgentTypeSchema {
            type_name: AgentTypeName(type_name.to_string()),
            description: String::new(),
            source_language: String::new(),
            schema: SchemaGraph::empty(),
            constructor: AgentConstructorSchema {
                name: None,
                description: String::new(),
                prompt_hint: None,
                input_schema: InputSchema::parameters(vec![]),
            },
            methods: vec![],
            dependencies: vec![],
            mode: AgentMode::Ephemeral,
            http_mount: None,
            snapshotting: Snapshotting::Disabled(Empty {}),
            config: vec![],
        }
    }
}

// TODO: use published version when available
enum GolemDependencySource {
    Path(std::path::PathBuf),
    GitMain,
}

impl GolemDependencySource {
    fn dep_item(&self, crate_path: &str, features: &[&str]) -> anyhow::Result<Item> {
        match self {
            GolemDependencySource::Path(repo_root) => {
                let dependency_path = repo_root.join(crate_path);
                Ok(path_dep(fs::path_to_str(&dependency_path)?, features))
            }
            GolemDependencySource::GitMain => Ok(git_dep(
                "https://github.com/golemcloud/golem",
                "main",
                features,
            )),
        }
    }
}

fn add_features(entry: &mut InlineTable, features: &[&str]) {
    if !features.is_empty() {
        let mut feature_items = Array::default();
        for feature in features {
            feature_items.push(*feature);
        }
        entry.insert("default-features", Value::from(false));
        entry.insert("features", Value::Array(feature_items));
    }
}

fn dep(version: &str, features: &[&str]) -> Item {
    if features.is_empty() {
        return value(version);
    }

    let mut entry = InlineTable::new();
    entry.insert("version", Value::from(version));
    add_features(&mut entry, features);
    Item::Value(Value::InlineTable(entry))
}

fn git_dep(url: &str, branch: &str, features: &[&str]) -> Item {
    let mut entry = InlineTable::new();
    entry.insert("git", Value::from(url));
    entry.insert("branch", Value::from(branch));
    add_features(&mut entry, features);
    Item::Value(Value::InlineTable(entry))
}

fn path_dep(path: &str, features: &[&str]) -> Item {
    let mut entry = InlineTable::new();
    entry.insert("path", Value::from(path));
    add_features(&mut entry, features);
    Item::Value(Value::InlineTable(entry))
}
