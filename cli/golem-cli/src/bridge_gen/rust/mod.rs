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

use crate::bridge_gen::rust::rust::to_rust_ident;
use crate::bridge_gen::type_naming::{TypeNaming, user_supplied_fields};
use crate::bridge_gen::{BridgeGenerator, bridge_client_directory_name};
use crate::fs;
use crate::sdk_overrides::{sdk_overrides, workspace_root};
use anyhow::{anyhow, bail};
use camino::{Utf8Path, Utf8PathBuf};
use golem_common::model::agent::{AgentConfigSource, AgentMode};
use golem_common::schema::agent::{
    AgentConfigDeclarationSchema, AgentMethodSchema, AgentTypeSchema, InputSchema, OutputSchema,
};
use golem_common::schema::graph::SchemaTypeDef;
use golem_common::schema::multimodal::multimodal_variant_cases;
use golem_common::schema::schema_type::{
    BinaryRestrictions, SchemaType, TextRestrictions, VariantCaseType,
};
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
mod type_name;

pub use type_name::RustTypeName;

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
        let same_language = agent_type.source_language.eq_ignore_ascii_case("rust");
        let type_naming = TypeNaming::new(&agent_type, same_language)?;

        Ok(Self {
            target_path: target_path.to_path_buf(),
            agent_type,
            testing,
            same_language,
            type_naming,
            generated_language_enums: Vec::new(),
            generated_mimetypes_enums: Vec::new(),
            known_multimodals: Vec::new(),
        })
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
        doc["dependencies"]["golem-client"] = golem_source.dep_item("golem-client", &[])?;
        doc["dependencies"]["golem-common"] = golem_source.dep_item("golem-common", &["client"])?;
        doc["dependencies"]["reqwest"] = dep("0.13", &["rustls"]);
        doc["dependencies"]["reqwest-middleware"] = dep("0.5", &[]);
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
                    let __config_value: golem_common::schema::SchemaValue = (|| -> Result<golem_common::schema::SchemaValue, String> {
                        #value_encode
                    })().map_err(|__e| golem_client::bridge::ClientError::InvocationFailed { message: format!("Failed to encode config value: {__e}") })?;
                    let __config_json = serde_json::to_value(&__config_value).map_err(|__e| golem_client::bridge::ClientError::InvocationFailed { message: format!("Failed to serialize config value: {__e}") })?;
                    agent_config.push(golem_client::model::AgentConfigEntryDto {
                        path: vec![#(#path_segments),*],
                        value: __config_json.into(),
                    });
                }
            });
        }

        let get_with_config_method = if self.agent_type.mode == AgentMode::Durable {
            quote! {
                pub async fn get_with_config(#(#constructor_param_defs,)* #(#config_param_defs,)*) -> Result<Self, golem_client::bridge::ClientError> {
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

                pub async fn get_phantom_with_config(uuid: uuid::Uuid, #(#constructor_param_defs,)* #(#config_param_defs,)*) -> Result<Self, golem_client::bridge::ClientError> {
                    let constructor_parameters: serde_json::Value = #constructor_params_value;
                    let mut agent_config = Vec::new();
                    #(#config_encode_stmts)*
                    Self::__create(constructor_parameters, Some(uuid), agent_config).await
                }

                pub async fn new_phantom_with_config(#(#constructor_param_defs,)* #(#config_param_defs,)*) -> Result<Self, golem_client::bridge::ClientError> {
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
                pub async fn get(#(#constructor_param_defs),*) -> Result<Self, golem_client::bridge::ClientError> {
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

        let tokens = quote! {
            #![allow(unused)]
            #![allow(non_snake_case)]
            #![allow(clippy::all)]

            #[derive(Debug, Clone)]
            pub struct #client_struct_name {
                constructor_parameters: serde_json::Value,
                phantom_id: Option<uuid::Uuid>,
                agent_id: golem_client::model::AgentId,
            }

            impl #client_struct_name {
                #get_method

                pub async fn get_phantom(uuid: uuid::Uuid, #(#constructor_param_defs),*) -> Result<Self, golem_client::bridge::ClientError> {
                    let constructor_parameters: serde_json::Value = #constructor_params_value;
                    Self::__create(constructor_parameters, Some(uuid), vec![]).await
                }

                pub async fn new_phantom(#(#constructor_param_defs),*) -> Result<Self, golem_client::bridge::ClientError> {
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
                ) -> Result<Self, golem_client::bridge::ClientError> {
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
                ) -> Result<Option<golem_client::model::TypedSchemaValue>, golem_client::bridge::ClientError> {
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

    fn method_ident(&self, method: &AgentMethodSchema) -> Ident {
        Ident::new(&self.to_rust_ident(&method.name), Span::call_site())
    }

    fn trigger_method_ident(&self, method: &AgentMethodSchema) -> Ident {
        Ident::new(
            &format!("trigger_{}", self.to_rust_ident(&method.name)),
            Span::call_site(),
        )
    }

    fn schedule_method_ident(&self, method: &AgentMethodSchema) -> Ident {
        Ident::new(
            &format!("schedule_{}", self.to_rust_ident(&method.name)),
            Span::call_site(),
        )
    }

    fn internal_method_ident(&self, method: &AgentMethodSchema) -> Ident {
        Ident::new(
            &format!("__{}", self.to_rust_ident(&method.name)),
            Span::call_site(),
        )
    }

    fn await_method(&mut self, method: &AgentMethodSchema) -> anyhow::Result<TokenStream> {
        let name = self.method_ident(method);
        let internal_name = self.internal_method_ident(method);
        let return_type = self.output_return_type(&method.output_schema)?;
        let param_defs = self.input_param_defs(&method.input_schema)?;
        let param_refs = self.input_param_refs(&method.input_schema)?;

        match return_type {
            Some(return_type) => Ok(quote! {
                pub async fn #name(&self, #(#param_defs),*) -> Result<#return_type, golem_client::bridge::ClientError> {
                    let result = self.#internal_name(golem_client::model::AgentInvocationMode::Await, None, #(#param_refs),*).await?;
                    let result = result.unwrap(); // always Some because of Await
                    Ok(result)
                }
            }),
            None => Ok(quote! {
                pub async fn #name(&self, #(#param_defs),*) -> Result<(), golem_client::bridge::ClientError> {
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

        Ok(quote! {
            pub async fn #name(&self, #(#param_defs),*) -> Result<(), golem_client::bridge::ClientError> {
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

        Ok(quote! {
            pub async fn #name(&self, when: chrono::DateTime<chrono::Utc>, #(#param_defs),*) -> Result<(), golem_client::bridge::ClientError> {
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

        match return_type {
            Some(return_type) => {
                let decode_body = self.output_decode_expr(&method.output_schema)?;
                Ok(quote! {
                    async fn #name(&self, mode: golem_client::model::AgentInvocationMode, when: Option<chrono::DateTime<chrono::Utc>>, #(#param_defs),*) -> Result<Option<#return_type>, golem_client::bridge::ClientError> {
                        let method_parameters: serde_json::Value = #params_value;
                        let response = self.invoke(#name_lit, method_parameters, mode, when).await?;
                        match response {
                            Some(__typed) => {
                                let (_, __value) = __typed.into_parts();
                                let __decoded: #return_type = (|| -> Result<#return_type, String> {
                                    #decode_body
                                })().map_err(|__e| golem_client::bridge::ClientError::InvocationFailed { message: format!("Failed to decode result value: {__e}") })?;
                                Ok(Some(__decoded))
                            }
                            None => Ok(None),
                        }
                    }
                })
            }
            None => Ok(quote! {
                async fn #name(&self, mode: golem_client::model::AgentInvocationMode, when: Option<chrono::DateTime<chrono::Utc>>, #(#param_defs),*) -> Result<Option<()>, golem_client::bridge::ClientError> {
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
        match self.rust_input(input)? {
            RustInput::Params(params) => {
                let mut result = Vec::new();
                for (name, schema) in &params {
                    let ident = Ident::new(&self.to_rust_ident(name), Span::call_site());
                    let typ = self.type_reference(schema, false)?;
                    result.push(quote! { #ident: #typ });
                }
                Ok(result)
            }
            RustInput::Multimodal(cases) => {
                let name = self.get_or_create_multimodal(&cases);
                let name = Ident::new(&name, Span::call_site());
                Ok(vec![quote! { values: Vec<#name> }])
            }
        }
    }

    /// Bare parameter idents for forwarding to the internal method.
    fn input_param_refs(&mut self, input: &InputSchema) -> anyhow::Result<Vec<TokenStream>> {
        match self.rust_input(input)? {
            RustInput::Params(params) => Ok(params
                .iter()
                .map(|(name, _)| {
                    let ident = Ident::new(&self.to_rust_ident(name), Span::call_site());
                    quote! { #ident }
                })
                .collect()),
            RustInput::Multimodal(_) => Ok(vec![quote! { values }]),
        }
    }

    /// Block expression of type `serde_json::Value` encoding the input
    /// parameters into a schema-native `record` and serializing it.
    fn input_param_value(&mut self, input: &InputSchema) -> anyhow::Result<TokenStream> {
        let record_body = match self.rust_input(input)? {
            RustInput::Params(params) => {
                let mut field_encs = Vec::new();
                for (name, schema) in &params {
                    let ident = Ident::new(&self.to_rust_ident(name), Span::call_site());
                    let enc = self.emit_encode_expr(quote! { #ident }, schema, false, 0)?;
                    field_encs.push(quote! { #enc? });
                }
                quote! {
                    Ok(golem_common::schema::SchemaValue::Record { fields: vec![#(#field_encs),*] })
                }
            }
            RustInput::Multimodal(cases) => {
                let list = self.multimodal_list_encode(&cases, quote! { values })?;
                quote! {
                    Ok(golem_common::schema::SchemaValue::Record { fields: vec![#list?] })
                }
            }
        };

        Ok(quote! {
            {
                let __sv: golem_common::schema::SchemaValue = (|| -> Result<golem_common::schema::SchemaValue, String> {
                    #record_body
                })().map_err(|__e| golem_client::bridge::ClientError::InvocationFailed { message: format!("Failed to encode parameters: {__e}") })?;
                serde_json::to_value(&__sv).map_err(|__e| golem_client::bridge::ClientError::InvocationFailed { message: format!("Failed to serialize parameters: {__e}") })?
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
                .collect::<Result<Vec<golem_common::schema::SchemaValue>, String>>()
                .map(|__elems| golem_common::schema::SchemaValue::List { elements: __elems })
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
                golem_common::schema::SchemaValue::List { elements } => {
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
                    #enum_ident::#case_ident(__inner) => Ok(golem_common::schema::SchemaValue::Variant(golem_common::schema::VariantValuePayload {
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

                fn #encode_fn(value: #enum_ident) -> Result<golem_common::schema::SchemaValue, String> {
                    match value {
                        #(#encode_arms)*
                    }
                }

                fn #decode_fn(value: golem_common::schema::SchemaValue) -> Result<#enum_ident, String> {
                    match value {
                        golem_common::schema::SchemaValue::Variant(golem_common::schema::VariantValuePayload { case, payload }) => match case {
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

                fn #encode_fn(value: #name_ident) -> Result<golem_common::schema::SchemaValue, String> {
                    #encode_body
                }

                fn #decode_fn(value: golem_common::schema::SchemaValue) -> Result<#name_ident, String> {
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
                    Ok(golem_common::schema::SchemaValue::Record { fields: vec![#(#field_encs),*] })
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
                                #name::#case_ident(__inner) => Ok(golem_common::schema::SchemaValue::Variant(golem_common::schema::VariantValuePayload {
                                    case: #idx_u32,
                                    payload: Some(Box::new(#enc?)),
                                })),
                            });
                        }
                        None => arms.push(quote! {
                            #name::#case_ident => Ok(golem_common::schema::SchemaValue::Variant(golem_common::schema::VariantValuePayload {
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
                        #name::#case_ident => Ok(golem_common::schema::SchemaValue::Enum { case: #idx_u32 }),
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
                    Ok(golem_common::schema::SchemaValue::Flags { bits: vec![#(#flag_idents),*] })
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
                        #name::#branch_ident(__inner) => Ok(golem_common::schema::SchemaValue::Union(golem_common::schema::UnionValuePayload {
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
                        golem_common::schema::SchemaValue::Record { fields } => {
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
                        golem_common::schema::SchemaValue::Variant(golem_common::schema::VariantValuePayload { case, payload }) => match case {
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
                        golem_common::schema::SchemaValue::Enum { case } => match case {
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
                        golem_common::schema::SchemaValue::Flags { bits } => {
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
                        golem_common::schema::SchemaValue::Union(golem_common::schema::UnionValuePayload { tag, body }) => {
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
        let is_unstructured = {
            let graph = self.type_naming.graph();
            unstructured_text_restrictions(graph, typ)?.is_some()
                || unstructured_binary_restrictions(graph, typ)?.is_some()
        };
        if is_unstructured {
            return Ok(quote! { #val.to_schema_value() });
        }
        let e = Ident::new(&format!("__e{depth}"), Span::call_site());
        let next = depth + 1;
        let rendered = match typ {
            SchemaType::Bool { .. } => quote! { Ok(golem_common::schema::SchemaValue::Bool(#val)) },
            SchemaType::S8 { .. } => quote! { Ok(golem_common::schema::SchemaValue::S8(#val)) },
            SchemaType::S16 { .. } => quote! { Ok(golem_common::schema::SchemaValue::S16(#val)) },
            SchemaType::S32 { .. } => quote! { Ok(golem_common::schema::SchemaValue::S32(#val)) },
            SchemaType::S64 { .. } => quote! { Ok(golem_common::schema::SchemaValue::S64(#val)) },
            SchemaType::U8 { .. } => quote! { Ok(golem_common::schema::SchemaValue::U8(#val)) },
            SchemaType::U16 { .. } => quote! { Ok(golem_common::schema::SchemaValue::U16(#val)) },
            SchemaType::U32 { .. } => quote! { Ok(golem_common::schema::SchemaValue::U32(#val)) },
            SchemaType::U64 { .. } => quote! { Ok(golem_common::schema::SchemaValue::U64(#val)) },
            SchemaType::F32 { .. } => quote! { Ok(golem_common::schema::SchemaValue::F32(#val)) },
            SchemaType::F64 { .. } => quote! { Ok(golem_common::schema::SchemaValue::F64(#val)) },
            SchemaType::Char { .. } => quote! { Ok(golem_common::schema::SchemaValue::Char(#val)) },
            SchemaType::String { .. } => {
                quote! { Ok(golem_common::schema::SchemaValue::String(#val)) }
            }
            SchemaType::Option { inner, .. } => {
                let inner_enc = self.emit_encode_expr(quote! { #e }, inner, box_recursive, next)?;
                quote! {
                    match #val {
                        Some(#e) => Ok(golem_common::schema::SchemaValue::Option { inner: Some(Box::new(#inner_enc?)) }),
                        None => Ok(golem_common::schema::SchemaValue::Option { inner: None }),
                    }
                }
            }
            SchemaType::List { element, .. } => {
                let inner_enc = self.emit_encode_expr(quote! { #e }, element, false, next)?;
                quote! {
                    #val
                        .into_iter()
                        .map(|#e| #inner_enc)
                        .collect::<Result<Vec<golem_common::schema::SchemaValue>, String>>()
                        .map(|__elems| golem_common::schema::SchemaValue::List { elements: __elems })
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
                            .collect::<Result<Vec<golem_common::schema::SchemaValue>, String>>()?;
                        if __elems.len() != #len {
                            Err(format!("Expected fixed-list of length {}, got {}", #len, __elems.len()))
                        } else {
                            Ok(golem_common::schema::SchemaValue::FixedList { elements: __elems })
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
                        .map(|(#k, #v)| Ok::<(golem_common::schema::SchemaValue, golem_common::schema::SchemaValue), String>((#key_enc?, #val_enc?)))
                        .collect::<Result<Vec<(golem_common::schema::SchemaValue, golem_common::schema::SchemaValue)>, String>>()
                        .map(|__entries| golem_common::schema::SchemaValue::Map { entries: __entries })
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
                        Ok(golem_common::schema::SchemaValue::Tuple { elements: vec![#(#parts),*] })
                    }
                }
            }
            SchemaType::Result { spec, .. } => {
                let ok_arm = match spec.ok.as_deref() {
                    Some(ok_type) => {
                        let enc =
                            self.emit_encode_expr(quote! { __r }, ok_type, box_recursive, next)?;
                        quote! { Ok(__r) => Ok(golem_common::schema::SchemaValue::Result(golem_common::schema::ResultValuePayload::Ok { value: Some(Box::new(#enc?)) })), }
                    }
                    None => {
                        quote! { Ok(_) => Ok(golem_common::schema::SchemaValue::Result(golem_common::schema::ResultValuePayload::Ok { value: None })), }
                    }
                };
                let err_arm = match spec.err.as_deref() {
                    Some(err_type) => {
                        let enc =
                            self.emit_encode_expr(quote! { __r }, err_type, box_recursive, next)?;
                        quote! { Err(__r) => Ok(golem_common::schema::SchemaValue::Result(golem_common::schema::ResultValuePayload::Err { value: Some(Box::new(#enc?)) })), }
                    }
                    None => {
                        quote! { Err(_) => Ok(golem_common::schema::SchemaValue::Result(golem_common::schema::ResultValuePayload::Err { value: None })), }
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
                quote! { Ok(golem_common::schema::SchemaValue::Path { path: #val }) }
            }
            SchemaType::Url { .. } => {
                quote! { Ok(golem_common::schema::SchemaValue::Url { url: #val }) }
            }
            SchemaType::Datetime { .. } => {
                quote! {
                    chrono::DateTime::parse_from_rfc3339(&#val)
                        .map(|__dt| golem_common::schema::SchemaValue::Datetime { value: __dt.with_timezone(&chrono::Utc) })
                        .map_err(|__err| format!("Invalid RFC3339 datetime: {__err}"))
                }
            }
            SchemaType::Duration { .. } => {
                quote! { Ok(golem_common::schema::SchemaValue::Duration(golem_common::schema::DurationValuePayload { nanoseconds: #val })) }
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
            return Ok(quote! { <#ty>::from_schema_value(#val) });
        }
        let binary_restrictions = {
            let graph = self.type_naming.graph();
            unstructured_binary_restrictions(graph, typ)?.cloned()
        };
        if let Some(restrictions) = binary_restrictions {
            let ty = self.unstructured_binary_type(&restrictions);
            return Ok(quote! { <#ty>::from_schema_value(#val) });
        }
        let e = Ident::new(&format!("__e{depth}"), Span::call_site());
        let next = depth + 1;
        let rendered = match typ {
            SchemaType::Bool { .. } => quote! {
                match #val { golem_common::schema::SchemaValue::Bool(__b) => Ok(__b), __other => Err(format!("Expected bool value, got {:?}", __other)) }
            },
            SchemaType::S8 { .. } => quote! {
                match #val { golem_common::schema::SchemaValue::S8(__b) => Ok(__b), __other => Err(format!("Expected s8 value, got {:?}", __other)) }
            },
            SchemaType::S16 { .. } => quote! {
                match #val { golem_common::schema::SchemaValue::S16(__b) => Ok(__b), __other => Err(format!("Expected s16 value, got {:?}", __other)) }
            },
            SchemaType::S32 { .. } => quote! {
                match #val { golem_common::schema::SchemaValue::S32(__b) => Ok(__b), __other => Err(format!("Expected s32 value, got {:?}", __other)) }
            },
            SchemaType::S64 { .. } => quote! {
                match #val { golem_common::schema::SchemaValue::S64(__b) => Ok(__b), __other => Err(format!("Expected s64 value, got {:?}", __other)) }
            },
            SchemaType::U8 { .. } => quote! {
                match #val { golem_common::schema::SchemaValue::U8(__b) => Ok(__b), __other => Err(format!("Expected u8 value, got {:?}", __other)) }
            },
            SchemaType::U16 { .. } => quote! {
                match #val { golem_common::schema::SchemaValue::U16(__b) => Ok(__b), __other => Err(format!("Expected u16 value, got {:?}", __other)) }
            },
            SchemaType::U32 { .. } => quote! {
                match #val { golem_common::schema::SchemaValue::U32(__b) => Ok(__b), __other => Err(format!("Expected u32 value, got {:?}", __other)) }
            },
            SchemaType::U64 { .. } => quote! {
                match #val { golem_common::schema::SchemaValue::U64(__b) => Ok(__b), __other => Err(format!("Expected u64 value, got {:?}", __other)) }
            },
            SchemaType::F32 { .. } => quote! {
                match #val { golem_common::schema::SchemaValue::F32(__b) => Ok(__b), __other => Err(format!("Expected f32 value, got {:?}", __other)) }
            },
            SchemaType::F64 { .. } => quote! {
                match #val { golem_common::schema::SchemaValue::F64(__b) => Ok(__b), __other => Err(format!("Expected f64 value, got {:?}", __other)) }
            },
            SchemaType::Char { .. } => quote! {
                match #val { golem_common::schema::SchemaValue::Char(__b) => Ok(__b), __other => Err(format!("Expected char value, got {:?}", __other)) }
            },
            SchemaType::String { .. } => quote! {
                match #val { golem_common::schema::SchemaValue::String(__b) => Ok(__b), __other => Err(format!("Expected string value, got {:?}", __other)) }
            },
            SchemaType::Option { inner, .. } => {
                let inner_dec = self.emit_decode_expr(quote! { #e }, inner, box_recursive, next)?;
                quote! {
                    match #val {
                        golem_common::schema::SchemaValue::Option { inner } => match inner {
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
                        golem_common::schema::SchemaValue::List { elements } => {
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
                        golem_common::schema::SchemaValue::FixedList { elements } => {
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
                        golem_common::schema::SchemaValue::Map { entries } => {
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
                            golem_common::schema::SchemaValue::Tuple { elements } if elements.is_empty() => Ok(()),
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
                            golem_common::schema::SchemaValue::Tuple { elements } => {
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
                            golem_common::schema::SchemaValue::Result(golem_common::schema::ResultValuePayload::Ok { value }) => {
                                let __ov = *value.ok_or_else(|| "Missing ok value".to_string())?;
                                Ok(Ok(#dec?))
                            }
                        }
                    }
                    None => quote! {
                        golem_common::schema::SchemaValue::Result(golem_common::schema::ResultValuePayload::Ok { .. }) => Ok(Ok(())),
                    },
                };
                let err_arm = match spec.err.as_deref() {
                    Some(err_type) => {
                        let dec =
                            self.emit_decode_expr(quote! { __ev }, err_type, box_recursive, next)?;
                        quote! {
                            golem_common::schema::SchemaValue::Result(golem_common::schema::ResultValuePayload::Err { value }) => {
                                let __ev = *value.ok_or_else(|| "Missing err value".to_string())?;
                                Ok(Err(#dec?))
                            }
                        }
                    }
                    None => quote! {
                        golem_common::schema::SchemaValue::Result(golem_common::schema::ResultValuePayload::Err { .. }) => Ok(Err(())),
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
                match #val { golem_common::schema::SchemaValue::Path { path } => Ok(path), __other => Err(format!("Expected path value, got {:?}", __other)) }
            },
            SchemaType::Url { .. } => quote! {
                match #val { golem_common::schema::SchemaValue::Url { url } => Ok(url), __other => Err(format!("Expected url value, got {:?}", __other)) }
            },
            SchemaType::Datetime { .. } => quote! {
                match #val { golem_common::schema::SchemaValue::Datetime { value } => Ok(value.to_rfc3339()), __other => Err(format!("Expected datetime value, got {:?}", __other)) }
            },
            SchemaType::Duration { .. } => quote! {
                match #val { golem_common::schema::SchemaValue::Duration(__p) => Ok(__p.nanoseconds), __other => Err(format!("Expected duration value, got {:?}", __other)) }
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
                quote! { golem_common::agentic::UnstructuredText<#enum_ty> }
            }
            _ => quote! { golem_common::agentic::UnstructuredText },
        }
    }

    fn unstructured_binary_type(&mut self, restrictions: &BinaryRestrictions) -> TokenStream {
        match &restrictions.mime_types {
            Some(mimes) if !mimes.is_empty() => {
                let enum_ty = self.get_mimetypes_enum(mimes);
                quote! { golem_common::agentic::UnstructuredBinary<#enum_ty> }
            }
            _ => quote! { golem_common::agentic::UnstructuredBinary },
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

                impl golem_common::agentic::AllowedLanguages for #ident {
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

                impl golem_common::agentic::AllowedMimeTypes for #ident {
                    fn all() -> &'static [&'static str] {
                        &[#(#code_strings),*]
                    }

                    fn from_mime_type(mime_type: &str) -> Option<Self> {
                        match mime_type {
                            #(#from_cases,)*
                            _ => None,
                        }
                    }

                    fn to_mime_type(&self) -> String {
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

    // --- Identifier helpers -------------------------------------------------

    fn to_rust_ident(&self, name: &str) -> String {
        to_rust_ident(name, self.same_language)
    }

    fn to_rust_case_name(&self, name: &str) -> String {
        if self.same_language {
            to_rust_ident(name, true)
        } else {
            to_rust_ident(name, false).to_upper_camel_case()
        }
    }

    fn package_crate_name(&self) -> String {
        bridge_client_directory_name(&self.agent_type.type_name)
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
