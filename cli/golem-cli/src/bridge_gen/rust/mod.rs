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

use crate::bridge_gen::rust::rust::to_rust_ident;
use crate::bridge_gen::type_naming::TypeNaming;
use crate::bridge_gen::{BridgeGenerator, BridgeGeneratorConfig, bridge_client_directory_name};
use crate::fs;
use crate::sdk_overrides::{sdk_overrides, workspace_root};
use anyhow::anyhow;
use camino::{Utf8Path, Utf8PathBuf};
use golem_common::model::agent::{
    AgentConfigDeclaration, AgentConfigSource, AgentMethod, AgentType, BinaryType, DataSchema,
    ElementSchema, NamedElementSchemas, TextType,
};
use golem_wasm::analysis::AnalysedType;
use heck::{ToSnakeCase, ToUpperCamelCase};
use proc_macro2::{Ident, Span, TokenStream};
use quote::quote;
use std::collections::{BTreeMap, HashMap};
use syn::{Lit, LitStr};
use toml_edit::{Array, DocumentMut, InlineTable, Item, Table, Value, value};
use tracing::debug;

#[allow(clippy::module_inception)]
mod rust;
mod type_name;

pub use type_name::RustTypeName;

#[allow(dead_code)]
pub struct RustBridgeGenerator {
    target_path: Utf8PathBuf,
    agent_type: AgentType,
    testing: bool,
    same_language: bool,
    config: BridgeGeneratorConfig,

    type_naming: TypeNaming<RustTypeName>,
    // TODO: we should integrate these names with type naming to avoid collisions
    generated_language_enums: BTreeMap<Vec<TextType>, String>,
    generated_mimetypes_enums: BTreeMap<Vec<BinaryType>, String>,
    known_multimodals: HashMap<NamedElementSchemas, String>,
}

impl BridgeGenerator for RustBridgeGenerator {
    fn new(
        agent_type: AgentType,
        target_path: &Utf8Path,
        testing: bool,
        config: BridgeGeneratorConfig,
    ) -> anyhow::Result<Self> {
        let same_language = agent_type.source_language.eq_ignore_ascii_case("rust");
        let type_naming = TypeNaming::new(&agent_type, same_language)?;

        Ok(Self {
            target_path: target_path.to_path_buf(),
            agent_type,
            testing,
            same_language,
            config,

            type_naming,
            generated_language_enums: BTreeMap::new(),
            generated_mimetypes_enums: BTreeMap::new(),
            known_multimodals: HashMap::new(),
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
    /// Returns derive attributes for a generated type, conditionally including serde
    /// derives when the `serde` feature is enabled in the generated crate.
    ///
    /// Evaluates `BridgeGeneratorConfig::derive_rules` against the type name: each rule
    /// whose regex pattern matches contributes its derives. All matching derives are
    /// merged and deduplicated before emission.
    fn base_derive_attrs(&self, type_name: &str) -> TokenStream {
        let mut derive_set = Vec::<String>::new();
        for rule in &self.config.derive_rules {
            if let Ok(re) = regex::Regex::new(&rule.pattern)
                && re.is_match(type_name)
            {
                for d in &rule.derives {
                    if !derive_set.contains(d) {
                        derive_set.push(d.clone());
                    }
                }
            }
        }

        let additional: Vec<TokenStream> = derive_set
            .iter()
            .filter_map(|d| syn::parse_str::<syn::Path>(d).ok())
            .map(|path| quote! { #path })
            .collect();

        if additional.is_empty() {
            quote! {
                #[derive(Debug, Clone)]
                #[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
            }
        } else {
            quote! {
                #[derive(Debug, Clone, #(#additional),*)]
                #[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
            }
        }
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

        let _package_name = self.package_name();

        let mut doc = DocumentMut::new();

        // Set up [package] section
        doc["package"] = Item::Table(Default::default());
        doc["package"]["name"] = value(self.package_crate_name());
        doc["package"]["version"] = value("0.0.1");
        doc["package"]["edition"] = value("2021");
        doc["package"]["description"] = value("Generated by golem-cli");

        doc["dependencies"] = Item::Table(Table::default());
        doc["dependencies"]["chrono"] = dep("0.4", &[]);
        doc["dependencies"]["nonempty-collections"] = dep("0.3.1", &[]);
        doc["dependencies"]["reqwest"] = dep("0.13", &["rustls"]);
        doc["dependencies"]["reqwest-middleware"] = dep("0.5", &[]);
        doc["dependencies"]["serde_json"] = dep("1", &[]);
        doc["dependencies"]["uuid"] = dep("1.18.1", &["v4"]);

        // Client-only deps (networking, Golem SDK) — optional, behind `client` feature
        fn optional_dep(version: &str, features: &[&str]) -> Item {
            let mut entry = Item::Table(Table::default());
            entry["version"] = value(version);
            if !features.is_empty() {
                let mut feature_items = Array::default();
                for feature in features {
                    feature_items.push(*feature);
                }
                entry["default-features"] = value(false);
                entry["features"] = value(feature_items);
            }
            entry["optional"] = value(true);
            entry
        }

        doc["dependencies"]["golem-client"] =
            golem_source.optional_dep_item("golem-client", &[])?;
        doc["dependencies"]["golem-common"] =
            golem_source.optional_dep_item("golem-common", &["client"])?;
        doc["dependencies"]["golem-wasm"] =
            golem_source.optional_dep_item("golem-wasm", &["client"])?;
        doc["dependencies"]["reqwest"] = optional_dep("0.13", &["rustls"]);
        doc["dependencies"]["reqwest-middleware"] = optional_dep("0.5", &[]);

        // Optional serde dependency for JSON serialization
        {
            let mut serde_entry = Item::Table(Table::default());
            serde_entry["version"] = value("1");
            let mut serde_features = Array::default();
            serde_features.push("derive");
            serde_entry["features"] = value(serde_features);
            serde_entry["optional"] = value(true);
            doc["dependencies"]["serde"] = serde_entry;
        }

        // [features] section
        doc["features"] = Item::Table(Table::default());
        let mut serde_feat = Array::default();
        serde_feat.push("dep:serde");
        doc["features"]["serde"] = value(serde_feat);

        let mut client_feat = Array::default();
        client_feat.push("dep:golem-client");
        client_feat.push("dep:golem-common");
        client_feat.push("dep:golem-wasm");
        client_feat.push("dep:reqwest");
        client_feat.push("dep:reqwest-middleware");
        doc["features"]["client"] = value(client_feat);

        let mut default_feat = Array::default();
        default_feat.push("client");
        doc["features"]["default"] = value(default_feat);

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
        let agent_type_name = &self.agent_type.type_name.0;
        let agent_type_name_lit = Lit::Str(LitStr::new(agent_type_name, Span::call_site()));
        let client_struct_name = Ident::new(agent_type_name, Span::call_site());

        let input_schema = self.agent_type.constructor.input_schema.clone();
        let constructor_params = self.parameter_list(&input_schema)?;

        let typed_constructor_parameters_ident =
            Ident::new("typed_constructor_parameters", Span::call_site());
        let constructor_params_to_data_value = self.encode_as_data_value(
            &typed_constructor_parameters_ident,
            &self.agent_type.constructor.input_schema,
        );

        let mut methods = Vec::new();
        for method in self.agent_type.methods.clone() {
            methods.extend(self.methods(&method)?);
        }

        let local_configs: Vec<&AgentConfigDeclaration> = self
            .agent_type
            .config
            .iter()
            .filter(|c| c.source == AgentConfigSource::Local)
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
            let param_type = self.wit_type_to_typeref(&config.value_type)?;
            config_param_defs.push(quote! { #param_name: Option<#param_type> });

            let path_segments: Vec<TokenStream> = config
                .path
                .iter()
                .map(|s| {
                    let lit = Lit::Str(LitStr::new(s, Span::call_site()));
                    quote! { #lit.to_string() }
                })
                .collect();
            config_encode_stmts.push(quote! {
                if let Some(value) = #param_name {
                    agent_config.push(golem_client::model::AgentConfigEntryDto {
                        path: vec![#(#path_segments),*],
                        value: serde_json::to_value(value).unwrap(),
                    });
                }
            });
        }

        let with_config_methods = if !local_configs.is_empty() {
            quote! {
                pub async fn get_with_config(#(#constructor_params,)* #(#config_param_defs,)*) -> Result<Self, golem_client::bridge::ClientError> {
                    #constructor_params_to_data_value
                    let constructor_parameters: golem_common::model::agent::UntypedJsonDataValue =
                        typed_constructor_parameters.into();
                    let mut agent_config = Vec::new();
                    #(#config_encode_stmts)*
                    Self::__create(constructor_parameters, None, agent_config).await
                }

                pub async fn get_phantom_with_config(uuid: uuid::Uuid, #(#constructor_params,)* #(#config_param_defs,)*) -> Result<Self, golem_client::bridge::ClientError> {
                    #constructor_params_to_data_value
                    let constructor_parameters: golem_common::model::agent::UntypedJsonDataValue =
                        typed_constructor_parameters.into();
                    let mut agent_config = Vec::new();
                    #(#config_encode_stmts)*
                    Self::__create(constructor_parameters, Some(uuid), agent_config).await
                }

                pub async fn new_phantom_with_config(#(#constructor_params,)* #(#config_param_defs,)*) -> Result<Self, golem_client::bridge::ClientError> {
                    #constructor_params_to_data_value
                    let constructor_parameters: golem_common::model::agent::UntypedJsonDataValue =
                        typed_constructor_parameters.into();
                    let mut agent_config = Vec::new();
                    #(#config_encode_stmts)*
                    Self::__create(constructor_parameters, Some(uuid::Uuid::new_v4()), agent_config).await
                }
            }
        } else {
            quote! {}
        };

        let global_config = self.global_config();

        let types = self.type_definitions()?;
        let multimodals = self.multimodals()?;
        let languages = self.languages_module();
        let mimetypes = self.mimetypes_module();

        let multimodal_import = if self.known_multimodals.is_empty() {
            quote! {}
        } else {
            quote! { use crate::multimodal::MultimodalEnum; }
        };

        let tokens = quote! {
            #![allow(unused)]

            #[cfg(feature = "client")]
            use golem_common::base_model::agent::{UnstructuredBinaryExtensions, UnstructuredTextExtensions};
            #[cfg(feature = "client")]
            use golem_wasm::{FromValueAndType, IntoValueAndType};
            #[cfg(feature = "client")]
            #multimodal_import

            #[cfg(feature = "client")]
            pub struct #client_struct_name {
                constructor_parameters: golem_client::model::UntypedJsonDataValue,
                phantom_id: Option<uuid::Uuid>,
                agent_id: golem_common::model::AgentId,
            }

            #[cfg(feature = "client")]
            impl std::fmt::Debug for #client_struct_name {
                fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                    f.debug_struct(stringify!(#client_struct_name))
                        .field("constructor_parameters", &self.constructor_parameters)
                        .field("phantom_id", &self.phantom_id)
                        .field("agent_id", &self.agent_id)
                        .finish()
                }
            }

            #[cfg(feature = "client")]
            impl #client_struct_name {
                pub async fn get(#(#constructor_params),*) -> Result<Self, golem_client::bridge::ClientError> {
                    #constructor_params_to_data_value
                    let constructor_parameters: golem_common::model::agent::UntypedJsonDataValue =
                        typed_constructor_parameters.into();
                    Self::__create(constructor_parameters, None, vec![]).await
                }

                pub async fn get_phantom(uuid: uuid::Uuid, #(#constructor_params),*) -> Result<Self, golem_client::bridge::ClientError> {
                    #constructor_params_to_data_value
                    let constructor_parameters: golem_common::model::agent::UntypedJsonDataValue =
                        typed_constructor_parameters.into();
                    Self::__create(constructor_parameters, Some(uuid), vec![]).await
                }

                pub async fn new_phantom(#(#constructor_params),*) -> Result<Self, golem_client::bridge::ClientError> {
                    #constructor_params_to_data_value
                    let constructor_parameters: golem_common::model::agent::UntypedJsonDataValue =
                        typed_constructor_parameters.into();
                    Self::__create(constructor_parameters, Some(uuid::Uuid::new_v4()), vec![]).await
                }

                #with_config_methods

                /// Returns the agent's identity, containing the component ID and agent name.
                pub fn agent_id(&self) -> &golem_common::model::AgentId {
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
                    constructor_parameters: golem_client::model::UntypedJsonDataValue,
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
                            agent_type_name: #agent_type_name_lit.to_string(),
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
                    method_parameters: golem_client::model::UntypedJsonDataValue,
                    mode: golem_client::model::AgentInvocationMode,
                    schedule_at: Option<chrono::DateTime<chrono::Utc>>,
                ) -> Result<Option<golem_client::model::UntypedJsonDataValue>, golem_client::bridge::ClientError> {
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
                            agent_type_name: #agent_type_name_lit.to_string(),
                            parameters: self.constructor_parameters.clone(),
                            phantom_id: self.phantom_id.clone(),
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

            #types

            #multimodals

            #languages

            #mimetypes
        };

        Ok(tokens)
    }

    fn get_or_create_multimodal(&mut self, elements: &NamedElementSchemas) -> String {
        let cnt = self.known_multimodals.len() + 1;
        self.known_multimodals
            .entry(elements.clone())
            .or_insert_with(|| format!("Multimodal{}", cnt))
            .clone()
    }

    fn parameter_list(&mut self, data_schema: &DataSchema) -> anyhow::Result<Vec<TokenStream>> {
        // each item is 'name: type'

        match data_schema {
            DataSchema::Tuple(elements) => {
                let mut result = Vec::new();
                for element in &elements.elements {
                    let name = Ident::new(&self.to_rust_ident(&element.name), Span::call_site());
                    let typ = self.element_schema_to_typeref(&element.schema)?;
                    result.push(quote! {#name: #typ});
                }
                Ok(result)
            }
            DataSchema::Multimodal(elements) => {
                let name = self.get_or_create_multimodal(elements);
                let name = Ident::new(&name, Span::call_site());
                Ok(vec![
                    quote! { values: crate::multimodal::Multimodal<crate::multimodal::#name> },
                ])
            }
        }
    }

    fn parameter_refs(&mut self, data_schema: &DataSchema) -> Vec<TokenStream> {
        // each item is 'name: type'

        match data_schema {
            DataSchema::Tuple(elements) => {
                let mut result = Vec::new();
                for element in &elements.elements {
                    let name = Ident::new(&self.to_rust_ident(&element.name), Span::call_site());
                    result.push(quote! {#name});
                }
                result
            }
            DataSchema::Multimodal(_elements) => {
                vec![quote! { values }]
            }
        }
    }

    fn get_languages_enum(&mut self, restrictions: &[TextType]) -> TokenStream {
        let restrictions = restrictions.to_vec();
        if let Some(existing) = self.generated_language_enums.get(&restrictions) {
            let ident = Ident::new(existing, Span::call_site());
            quote! { crate::languages::#ident }
        } else {
            let counter = self.generated_language_enums.len();
            let ident = Ident::new(&format!("Languages{}", counter), Span::call_site());
            self.generated_language_enums
                .insert(restrictions.clone(), ident.to_string());
            quote! { crate::languages::#ident }
        }
    }

    fn get_mimetypes_enum(&mut self, restrictions: &[BinaryType]) -> TokenStream {
        let restrictions = restrictions.to_vec();
        if let Some(existing) = self.generated_mimetypes_enums.get(&restrictions) {
            let ident = Ident::new(existing, Span::call_site());
            quote! { crate::mimetypes::#ident }
        } else {
            let counter = self.generated_mimetypes_enums.len();
            let ident = Ident::new(&format!("Mimetypes{}", counter), Span::call_site());
            self.generated_mimetypes_enums
                .insert(restrictions.clone(), ident.to_string());
            quote! { crate::mimetypes::#ident }
        }
    }

    fn element_schema_to_typeref(
        &mut self,
        element_schema: &ElementSchema,
    ) -> anyhow::Result<TokenStream> {
        match element_schema {
            ElementSchema::ComponentModel(schema) => self.wit_type_to_typeref(&schema.element_type),
            ElementSchema::UnstructuredText(descriptor) => {
                if let Some(restrictions) = &descriptor.restrictions {
                    let languages_enum = self.get_languages_enum(restrictions);
                    Ok(
                        quote! { golem_wasm::agentic::unstructured_text::UnstructuredText<#languages_enum> },
                    )
                } else {
                    Ok(quote! { golem_wasm::agentic::unstructured_text::UnstructuredText })
                }
            }
            ElementSchema::UnstructuredBinary(descriptor) => {
                if let Some(restrictions) = &descriptor.restrictions {
                    let mimetypes_enum = self.get_mimetypes_enum(restrictions);
                    Ok(
                        quote! { golem_wasm::agentic::unstructured_binary::UnstructuredBinary<#mimetypes_enum> },
                    )
                } else {
                    Ok(
                        quote! { golem_wasm::agentic::unstructured_binary::UnstructuredBinary<String> },
                    )
                }
            }
        }
    }

    fn wit_type_to_typedef(
        &self,
        type_name: &RustTypeName,
        typ: &AnalysedType,
    ) -> anyhow::Result<TokenStream> {
        let name = match type_name {
            RustTypeName::Derived(type_name) => Ident::new(type_name, Span::call_site()),
            RustTypeName::Remapped() => {
                todo!("implement remap")
            }
        };

        match typ {
            AnalysedType::Variant(variant) => {
                let mut cases = Vec::new();
                let mut into_value_cases = Vec::new();
                let mut from_value_cases = Vec::new();
                let mut case_names_lit = Vec::new();
                let mut case_type_tokens = Vec::new();

                for (idx, case) in variant.cases.iter().enumerate() {
                    let case_ident =
                        Ident::new(&self.to_rust_case_name(&case.name), Span::call_site());
                    let idx_u32 = idx as u32;
                    case_names_lit.push(case.name.clone());

                    match &case.typ {
                        Some(typ) => {
                            // TODO: auto-inline anonymous records

                            let inner = self.wit_type_to_typeref(typ)?;
                            cases.push(quote! { #case_ident(#inner) });

                            // get_type() case — include the inner type
                            case_type_tokens.push(
                                quote! { Some(<#inner as golem_wasm::IntoValue>::get_type()) },
                            );

                            // IntoValue implementation
                            into_value_cases.push(quote! {
                                Self::#case_ident(value) => golem_wasm::Value::Variant {
                                    case_idx: #idx_u32,
                                    case_value: Some(Box::new(value.into_value())),
                                }
                            });

                            // FromValue implementation
                            from_value_cases.push(quote! {
                                    #idx_u32 => {
                                        let inner_value = case_value.ok_or_else(|| format!("Expected case_value for {}", stringify!(#case_ident)))?;
                                        Ok(Self::#case_ident(<#inner as golem_wasm::FromValue>::from_value(*inner_value)?))
                                    }
                                });
                        }
                        None => {
                            cases.push(quote! { #case_ident });

                            // get_type() case — no inner type
                            case_type_tokens.push(quote! { None });

                            // IntoValue implementation
                            into_value_cases.push(quote! {
                                Self::#case_ident => golem_wasm::Value::Variant {
                                    case_idx: #idx_u32,
                                    case_value: None,
                                }
                            });

                            // FromValue implementation
                            from_value_cases.push(quote! {
                                #idx_u32 => Ok(Self::#case_ident)
                            });
                        }
                    }
                }

                let attrs = self.base_derive_attrs(&name.to_string());
                Ok(quote! {
                    #attrs
                    pub enum #name {
                        #(#cases),*
                    }

                    #[cfg(feature = "client")]
                    impl golem_wasm::IntoValue for #name {
                        fn into_value(self) -> golem_wasm::Value {
                            match self {
                                #(#into_value_cases),*
                            }
                        }

                        fn get_type() -> golem_wasm::analysis::AnalysedType {
                            golem_wasm::analysis::AnalysedType::Variant(golem_wasm::analysis::TypeVariant {
                                name: Some(stringify!(#name).to_string()),
                                owner: None,
                                cases: vec![
                                    #(
                                        golem_wasm::analysis::NameOptionTypePair {
                                            name: #case_names_lit.to_string(),
                                            typ: #case_type_tokens,
                                        }
                                    ),*
                                ],
                            })
                        }
                    }

                    #[cfg(feature = "client")]
                    impl golem_wasm::FromValue for #name {
                        fn from_value(value: golem_wasm::Value) -> Result<Self, String> {
                            match value {
                                golem_wasm::Value::Variant { case_idx, case_value } => {
                                    match case_idx {
                                        #(#from_value_cases,)*
                                        _ => Err(format!("Invalid variant case index: {}", case_idx)),
                                    }
                                }
                                _ => Err(format!("Expected Variant value, got {:?}", value)),
                            }
                        }
                    }
                })
            }
            AnalysedType::Result(result) => {
                let ok = match result.ok.as_ref() {
                    Some(ok_type) => self.wit_type_to_typeref(ok_type)?,
                    None => quote! { () },
                };
                let err = match result.err.as_ref() {
                    Some(err_type) => self.wit_type_to_typeref(err_type)?,
                    None => quote! { () },
                };
                Ok(quote! { pub type #name = Result<#ok, #err>; })
            }
            AnalysedType::Option(option) => {
                let inner = self.wit_type_to_typeref(&option.inner)?;
                Ok(quote! { pub type #name = Option<#inner>; })
            }
            AnalysedType::Enum(r#enum) => {
                let mut cases = Vec::new();
                let mut into_value_cases = Vec::new();
                let mut from_value_cases = Vec::new();
                let mut case_names_lit = Vec::new();

                for (idx, case) in r#enum.cases.iter().enumerate() {
                    let case_ident = Ident::new(&self.to_rust_case_name(case), Span::call_site());
                    cases.push(quote! { #case_ident });
                    case_names_lit.push(case.clone());

                    // IntoValue implementation
                    into_value_cases.push(quote! {
                        Self::#case_ident => golem_wasm::Value::Enum(#idx as u32)
                    });

                    // FromValue implementation
                    let idx_u32 = idx as u32;
                    from_value_cases.push(quote! {
                        #idx_u32 => Ok(Self::#case_ident)
                    });
                }

                let attrs = self.base_derive_attrs(&name.to_string());
                Ok(quote! {
                    #attrs
                    pub enum #name {
                        #(#cases),*
                    }

                    #[cfg(feature = "client")]
                    impl golem_wasm::IntoValue for #name {
                        fn into_value(self) -> golem_wasm::Value {
                            match self {
                                #(#into_value_cases),*
                            }
                        }

                        fn get_type() -> golem_wasm::analysis::AnalysedType {
                            golem_wasm::analysis::AnalysedType::Enum(golem_wasm::analysis::TypeEnum {
                                cases: vec![#(#case_names_lit.to_string()),*],
                                name: Some(stringify!(#name).to_string()),
                                owner: None,
                            })
                        }
                    }

                    #[cfg(feature = "client")]
                    impl golem_wasm::FromValue for #name {
                        fn from_value(value: golem_wasm::Value) -> Result<Self, String> {
                            match value {
                                golem_wasm::Value::Enum(idx) => {
                                    match idx {
                                        #(#from_value_cases,)*
                                        _ => Err(format!("Invalid enum index: {}", idx)),
                                    }
                                }
                                _ => Err(format!("Expected Enum value, got {:?}", value)),
                            }
                        }
                    }
                })
            }
            AnalysedType::Flags(_flags) => {
                Err(anyhow!("Flags are not supported")) // NOTE: none of the code-first SDKs support flags at the moment
            }
            AnalysedType::Record(record) => {
                let mut fields = Vec::new();
                let mut field_idents = Vec::new();
                let mut field_types = Vec::new();
                let mut field_names_lit = Vec::new();
                let mut into_value_fields = Vec::new();
                let mut from_value_fields = Vec::new();

                for field in &record.fields {
                    let field_ident =
                        Ident::new(&self.to_rust_ident(&field.name), Span::call_site());
                    let field_type = self.wit_type_to_typeref(&field.typ)?;

                    fields.push(quote! { pub #field_ident: #field_type });
                    field_idents.push(field_ident.clone());
                    field_types.push(field_type.clone());
                    field_names_lit.push(field.name.clone());

                    // IntoValue implementation
                    into_value_fields.push(quote! {
                        self.#field_ident.into_value()
                    });

                    // FromValue implementation
                    from_value_fields.push(quote! {
                            let #field_ident = <#field_type as golem_wasm::FromValue>::from_value(fields.remove(0))?;
                        });
                }

                let field_count = field_idents.len();

                let attrs = self.base_derive_attrs(&name.to_string());
                Ok(quote! {
                    #attrs
                    pub struct #name {
                        #(#fields),*
                    }

                    #[cfg(feature = "client")]
                    impl golem_wasm::IntoValue for #name {
                        fn into_value(self) -> golem_wasm::Value {
                            golem_wasm::Value::Record(vec![
                                #(#into_value_fields),*
                            ])
                        }

                        fn get_type() -> golem_wasm::analysis::AnalysedType {
                            use golem_wasm::analysis::analysed_type::field;

                            golem_wasm::analysis::AnalysedType::Record(golem_wasm::analysis::TypeRecord {
                                fields: vec![
                                    #(
                                        field(#field_names_lit, <#field_types as golem_wasm::IntoValue>::get_type())
                                    ),*
                                ],
                                name: Some(stringify!(#name).to_string()),
                                owner: None,
                            })
                        }
                    }

                    #[cfg(feature = "client")]
                    impl golem_wasm::FromValue for #name {
                        fn from_value(value: golem_wasm::Value) -> Result<Self, String> {
                            match value {
                                golem_wasm::Value::Record(mut fields) if fields.len() == #field_count => {
                                    #(#from_value_fields)*
                                    Ok(Self {
                                        #(#field_idents),*
                                    })
                                }
                                golem_wasm::Value::Record(fields) => {
                                    Err(format!("Expected Record with {} fields, got {}", #field_count, fields.len()))
                                }
                                _ => Err(format!("Expected Record value, got {:?}", value)),
                            }
                        }
                    }
                })
            }
            AnalysedType::Tuple(tuple) => {
                let mut elements = Vec::new();
                for item in tuple.items.iter() {
                    elements.push(self.wit_type_to_typeref(item)?);
                }
                Ok(quote! { pub type #name = (#(#elements),*); })
            }
            AnalysedType::List(list) => {
                let inner = self.wit_type_to_typeref(&list.inner)?;
                Ok(quote! { pub type #name = Vec<#inner>; })
            }
            AnalysedType::Str(_) => Ok(quote! { pub type #name = String; }),
            AnalysedType::Chr(_) => Ok(quote! { pub type #name = char; }),
            AnalysedType::F64(_) => Ok(quote! { pub type #name = f64; }),
            AnalysedType::F32(_) => Ok(quote! { pub type #name = f32; }),
            AnalysedType::U64(_) => Ok(quote! { pub type #name = u64; }),
            AnalysedType::S64(_) => Ok(quote! { pub type #name = i64; }),
            AnalysedType::U32(_) => Ok(quote! { pub type #name = u32; }),
            AnalysedType::S32(_) => Ok(quote! { pub type #name = i32; }),
            AnalysedType::U16(_) => Ok(quote! { pub type #name = u16; }),
            AnalysedType::S16(_) => Ok(quote! { pub type #name = i16; }),
            AnalysedType::U8(_) => Ok(quote! { pub type #name = u8; }),
            AnalysedType::S8(_) => Ok(quote! { pub type #name = i8; }),
            AnalysedType::Bool(_) => Ok(quote! { pub type #name = bool; }),
            AnalysedType::Handle(_) => Err(anyhow!("Handles are not supported")),
        }
    }

    fn wit_type_to_typeref(&self, typ: &AnalysedType) -> anyhow::Result<TokenStream> {
        let name = self.type_naming.type_name_for_type(typ);
        match name {
            Some(name) => match name {
                RustTypeName::Derived(name) => {
                    let name = Ident::new(name, Span::call_site());
                    Ok(quote! { #name })
                }
                RustTypeName::Remapped() => {
                    todo!("implement remap")
                }
            },
            None => match typ {
                AnalysedType::Option(inner) => {
                    let inner = self.wit_type_to_typeref(&inner.inner)?;
                    Ok(quote! { Option<#inner> })
                }
                AnalysedType::Str(_) => Ok(quote! { String }),
                AnalysedType::Chr(_) => Ok(quote! { char }),
                AnalysedType::F64(_) => Ok(quote! { f64 }),
                AnalysedType::F32(_) => Ok(quote! { f32 }),
                AnalysedType::U64(_) => Ok(quote! { u64 }),
                AnalysedType::S64(_) => Ok(quote! { i64 }),
                AnalysedType::U32(_) => Ok(quote! { u32 }),
                AnalysedType::S32(_) => Ok(quote! { i32 }),
                AnalysedType::U16(_) => Ok(quote! { u16 }),
                AnalysedType::S16(_) => Ok(quote! { i16 }),
                AnalysedType::U8(_) => Ok(quote! { u8 }),
                AnalysedType::S8(_) => Ok(quote! { i8 }),
                AnalysedType::Bool(_) => Ok(quote! { bool }),
                AnalysedType::Tuple(tuple) => {
                    let mut elements = Vec::new();
                    for item in tuple.items.iter() {
                        elements.push(self.wit_type_to_typeref(item)?);
                    }
                    Ok(quote! { (#(#elements),*) })
                }
                AnalysedType::List(list) => {
                    let inner = self.wit_type_to_typeref(&list.inner)?;
                    Ok(quote! { Vec<#inner> })
                }
                AnalysedType::Result(result) => {
                    let ok = match result.ok.as_ref() {
                        Some(ok) => self.wit_type_to_typeref(ok)?,
                        None => quote! { () },
                    };
                    let err = match result.err.as_ref() {
                        Some(err) => self.wit_type_to_typeref(err)?,
                        None => quote! { () },
                    };
                    Ok(quote! { Result<#ok, #err> })
                }
                AnalysedType::Handle(_)
                | AnalysedType::Variant(_)
                | AnalysedType::Enum(_)
                | AnalysedType::Flags(_)
                | AnalysedType::Record(_) => {
                    let type_name = self.type_naming.type_name_for_type(typ);
                    match type_name {
                        Some(RustTypeName::Derived(name)) => {
                            let name = Ident::new(name, Span::call_site());
                            Ok(quote! { #name })
                        }
                        Some(RustTypeName::Remapped()) => todo!("implement remap"),
                        None => Err(anyhow!("Missing type name for {:?}", typ)),
                    }
                }
            },
        }
    }

    fn encode_as_data_value(&self, name: &Ident, data_schema: &DataSchema) -> TokenStream {
        match data_schema {
            DataSchema::Tuple(elements) => {
                let encoded_elements = elements
                    .elements
                    .iter()
                    .map(|named_element| {
                        let name =
                            Ident::new(&self.to_rust_ident(&named_element.name), Span::call_site());
                        match &named_element.schema {
                            ElementSchema::ComponentModel(_) => {
                                quote! { golem_common::model::agent::ElementValue::ComponentModel(golem_common::model::agent::ComponentModelElementValue { value: #name.into_value_and_type() }) }
                            }
                            ElementSchema::UnstructuredText(_) => {
                                quote! { golem_common::model::agent::ElementValue::UnstructuredText(golem_common::model::agent::UnstructuredTextElementValue { value: #name.into_text_reference(), descriptor: golem_common::model::agent::TextDescriptor::default() }) }
                            }
                            ElementSchema::UnstructuredBinary(_) => {
                                quote! { golem_common::model::agent::ElementValue::UnstructuredBinary(golem_common::model::agent::UnstructuredBinaryElementValue { value: #name.into_binary_reference(), descriptor: golem_common::model::agent::BinaryDescriptor::default() }) }
                            }
                        }
                    })
                    .collect::<Vec<_>>();

                quote! {
                    let #name = golem_common::model::agent::DataValue::Tuple(
                        golem_common::model::agent::ElementValues {
                            elements: vec![#(#encoded_elements),*]
                        }
                    );
                }
            }
            DataSchema::Multimodal(_) => {
                quote! {
                    let #name = golem_common::model::agent::DataValue::Multimodal(
                        golem_common::model::agent::NamedElementValues {
                            elements: values.values.iter().map(|v| v.to_named_element_value()).collect()
                        }
                    );
                }
            }
        }
    }

    fn return_type_from_data_schema(
        &mut self,
        data_schema: &DataSchema,
    ) -> anyhow::Result<Option<TokenStream>> {
        match data_schema {
            DataSchema::Tuple(elements) => {
                if elements.elements.is_empty() {
                    Ok(None)
                } else if elements.elements.len() == 1 {
                    let element = &elements.elements[0];
                    Ok(Some(self.element_schema_to_typeref(&element.schema)?))
                } else {
                    Err(anyhow!("Multiple return values are not supported"))
                }
            }
            DataSchema::Multimodal(elements) => {
                let name = self.get_or_create_multimodal(elements);
                let name = Ident::new(&name, Span::call_site());
                Ok(Some(
                    quote! { multimodal::Multimodal<crate::multimodal::#name> },
                ))
            }
        }
    }

    fn methods(&mut self, method: &AgentMethod) -> anyhow::Result<Vec<TokenStream>> {
        Ok(vec![
            self.await_method(method)?,
            self.trigger_method(method)?,
            self.schedule_method(method)?,
            self.internal_method(method)?,
        ])
    }

    fn await_method_name(&self, method: &AgentMethod) -> Ident {
        let base_name = self.to_rust_ident(&method.name);
        Ident::new(&base_name, Span::call_site())
    }

    fn trigger_method_name(&self, method: &AgentMethod) -> Ident {
        let base_name = self.to_rust_ident(&method.name);
        Ident::new(&format!("trigger_{}", base_name), Span::call_site())
    }

    fn schedule_method_name(&self, method: &AgentMethod) -> Ident {
        let base_name = self.to_rust_ident(&method.name);
        Ident::new(&format!("schedule_{}", base_name), Span::call_site())
    }

    fn internal_method_name(&self, method: &AgentMethod) -> Ident {
        let base_name = self.to_rust_ident(&method.name);
        Ident::new(&format!("__{}", base_name), Span::call_site())
    }

    fn await_method(&mut self, method: &AgentMethod) -> anyhow::Result<TokenStream> {
        let name = self.await_method_name(method);
        let internal_name = self.internal_method_name(method);

        let return_type = self.return_type_from_data_schema(&method.output_schema)?;
        let param_defs = self.parameter_list(&method.input_schema)?;
        let param_refs = self.parameter_refs(&method.input_schema);

        match return_type {
            Some(return_type) => {
                Ok(quote! {
                    pub async fn #name(&self, #(#param_defs),*) -> Result<#return_type, golem_client::bridge::ClientError> {
                        let result = self.#internal_name(golem_client::model::AgentInvocationMode::Await, None, #(#param_refs),*).await?;
                        let result = result.unwrap(); // always Some because of Await
                        Ok(result)
                    }
                })
            }
            None => {
                Ok(quote! {
                    pub async fn #name(&self, #(#param_defs),*) -> Result<(), golem_client::bridge::ClientError> {
                        let result = self.#internal_name(golem_client::model::AgentInvocationMode::Await, None, #(#param_refs),*).await?;
                        let _result = result.unwrap(); // always Some because of Await
                        Ok(())
                    }
                })
            }
        }
    }

    fn trigger_method(&mut self, method: &AgentMethod) -> anyhow::Result<TokenStream> {
        let name = self.trigger_method_name(method);
        let internal_name = self.internal_method_name(method);

        let param_defs = self.parameter_list(&method.input_schema)?;
        let param_refs = self.parameter_refs(&method.input_schema);

        Ok(quote! {
            pub async fn #name(&self, #(#param_defs),*) -> Result<(), golem_client::bridge::ClientError> {
                let _ = self.#internal_name(golem_client::model::AgentInvocationMode::Schedule, None, #(#param_refs),*).await?;
                Ok(())
            }
        })
    }

    fn schedule_method(&mut self, method: &AgentMethod) -> anyhow::Result<TokenStream> {
        let name = self.schedule_method_name(method);
        let internal_name = self.internal_method_name(method);

        let param_defs = self.parameter_list(&method.input_schema)?;
        let param_refs = self.parameter_refs(&method.input_schema);

        Ok(quote! {
            pub async fn #name(&self, when: chrono::DateTime<chrono::Utc>, #(#param_defs),*) -> Result<(), golem_client::bridge::ClientError> {
                let _ = self.#internal_name(golem_client::model::AgentInvocationMode::Schedule, Some(when), #(#param_refs),*).await?;
                Ok(())
            }
        })
    }

    fn internal_method(&mut self, method: &AgentMethod) -> anyhow::Result<TokenStream> {
        let name_lit = Lit::Str(LitStr::new(&method.name, Span::call_site()));
        let name = self.internal_method_name(method);
        let param_defs = self.parameter_list(&method.input_schema)?;
        let return_type = self.return_type_from_data_schema(&method.output_schema)?;
        let typed_method_parameters_ident =
            Ident::new("typed_method_parameters", Span::call_site());
        let typed_method_parameters_to_data_value =
            self.encode_as_data_value(&typed_method_parameters_ident, &method.input_schema);

        let output_schema_as_value = self.schema_as_value(&method.output_schema);
        let decode_typed_data_value = self.decode_from_data_value(
            &Ident::new("typed_data_value", Span::call_site()),
            &method.output_schema,
        )?;

        match return_type {
            Some(return_type) => Ok(quote! {
                async fn #name(&self, mode: golem_client::model::AgentInvocationMode, when: Option<chrono::DateTime<chrono::Utc>>, #(#param_defs),*) -> Result<Option<#return_type>, golem_client::bridge::ClientError> {
                    #typed_method_parameters_to_data_value
                    let method_parameters: golem_common::model::agent::UntypedJsonDataValue = typed_method_parameters.into();
                    let response = self.invoke(#name_lit, method_parameters, mode, when).await?;
                    if let Some(untyped_data_value) = response {
                        let typed_data_value = golem_common::model::agent::DataValue::try_from_untyped_json(
                            untyped_data_value,
                            #output_schema_as_value
                        ).map_err(|err| golem_client::bridge::ClientError::InvocationFailed { message: format!("Failed to decode result value: {err}") })?;
                        #decode_typed_data_value
                    } else {
                        Ok(None)
                    }
                }
            }),
            None => Ok(quote! {
                async fn #name(&self, mode: golem_client::model::AgentInvocationMode, when: Option<chrono::DateTime<chrono::Utc>>, #(#param_defs),*) -> Result<Option<()>, golem_client::bridge::ClientError> {
                    #typed_method_parameters_to_data_value
                    let method_parameters: golem_common::model::agent::UntypedJsonDataValue = typed_method_parameters.into();
                    let response = self.invoke(#name_lit, method_parameters, mode, when).await?;
                    if let Some(untyped_data_value) = response {
                        let typed_data_value = golem_common::model::agent::DataValue::try_from_untyped_json(
                            untyped_data_value,
                            #output_schema_as_value
                        ).map_err(|err| golem_client::bridge::ClientError::InvocationFailed { message: format!("Failed to decode result value: {err}") })?;
                        Ok(Some(()))
                    } else {
                        Ok(None)
                    }
                }
            }),
        }
    }

    fn schema_as_value(&self, schema: &DataSchema) -> TokenStream {
        let named_element_schemas = self.named_element_schemas_as_value(match schema {
            DataSchema::Tuple(s) | DataSchema::Multimodal(s) => s,
        });

        match schema {
            DataSchema::Tuple(_) => {
                quote! {
                    golem_common::model::agent::DataSchema::Tuple(#named_element_schemas)
                }
            }
            DataSchema::Multimodal(_) => {
                quote! {
                    golem_common::model::agent::DataSchema::Multimodal(#named_element_schemas)
                }
            }
        }
    }

    fn named_element_schemas_as_value(&self, schemas: &NamedElementSchemas) -> TokenStream {
        let elements = schemas
            .elements
            .iter()
            .map(|elem| self.named_element_schema_as_value(elem))
            .collect::<Vec<_>>();

        quote! {
            golem_common::model::agent::NamedElementSchemas {
                elements: vec![#(#elements),*],
            }
        }
    }

    fn named_element_schema_as_value(
        &self,
        schema: &golem_common::model::agent::NamedElementSchema,
    ) -> TokenStream {
        let name = &schema.name;
        let element_schema = self.element_schema_as_value(&schema.schema);

        quote! {
            golem_common::model::agent::NamedElementSchema {
                name: #name.to_string(),
                schema: #element_schema,
            }
        }
    }

    fn element_schema_as_value(&self, schema: &ElementSchema) -> TokenStream {
        match schema {
            ElementSchema::ComponentModel(cm_schema) => {
                let element_type = self.analysed_type_as_value(&cm_schema.element_type);
                quote! {
                    golem_common::model::agent::ElementSchema::ComponentModel(
                        golem_common::model::agent::ComponentModelElementSchema {
                            element_type: #element_type,
                        },
                    )
                }
            }
            ElementSchema::UnstructuredText(text_descriptor) => {
                let restrictions = match &text_descriptor.restrictions {
                    Some(text_types) => {
                        let text_type_tokens = text_types
                            .iter()
                            .map(|tt| {
                                let language_code = &tt.language_code;
                                quote! {
                                    golem_common::model::agent::TextType {
                                        language_code: #language_code.to_string(),
                                    }
                                }
                            })
                            .collect::<Vec<_>>();
                        quote! {
                            Some(vec![#(#text_type_tokens),*])
                        }
                    }
                    None => quote! { None },
                };

                quote! {
                    golem_common::model::agent::ElementSchema::UnstructuredText(
                        golem_common::model::agent::TextDescriptor {
                            restrictions: #restrictions,
                        },
                    )
                }
            }
            ElementSchema::UnstructuredBinary(binary_descriptor) => {
                let restrictions = match &binary_descriptor.restrictions {
                    Some(binary_types) => {
                        let binary_type_tokens = binary_types
                            .iter()
                            .map(|bt| {
                                let mime_type = &bt.mime_type;
                                quote! {
                                    golem_common::model::agent::BinaryType {
                                        mime_type: #mime_type.to_string(),
                                    }
                                }
                            })
                            .collect::<Vec<_>>();
                        quote! {
                            Some(vec![#(#binary_type_tokens),*])
                        }
                    }
                    None => quote! { None },
                };

                quote! {
                    golem_common::model::agent::ElementSchema::UnstructuredBinary(
                        golem_common::model::agent::BinaryDescriptor {
                            restrictions: #restrictions,
                        },
                    )
                }
            }
        }
    }

    fn analysed_type_as_value(&self, analysed_type: &AnalysedType) -> TokenStream {
        match analysed_type {
            AnalysedType::Variant(tv) => {
                let name = tv
                    .name
                    .as_ref()
                    .map(|n| {
                        let lit = Lit::Str(LitStr::new(n, Span::call_site()));
                        quote! { Some(#lit.to_string()) }
                    })
                    .unwrap_or_else(|| quote! { None });
                let owner = tv
                    .owner
                    .as_ref()
                    .map(|n| {
                        let lit = Lit::Str(LitStr::new(n, Span::call_site()));
                        quote! { Some(#lit.to_string()) }
                    })
                    .unwrap_or_else(|| quote! { None });
                let cases = tv
                    .cases
                    .iter()
                    .map(|case| {
                        let case_name = &case.name;
                        let case_type = case.typ.as_ref().map(|t| self.analysed_type_as_value(t));
                        match case_type {
                            Some(typ) => {
                                quote! {
                                    golem_wasm::analysis::NameOptionTypePair {
                                        name: #case_name.to_string(),
                                        typ: Some(#typ),
                                    }
                                }
                            }
                            None => {
                                quote! {
                                    golem_wasm::analysis::NameOptionTypePair {
                                        name: #case_name.to_string(),
                                        typ: None,
                                    }
                                }
                            }
                        }
                    })
                    .collect::<Vec<_>>();
                quote! {
                    golem_wasm::analysis::AnalysedType::Variant(
                        golem_wasm::analysis::TypeVariant {
                            name: #name.clone(),
                            owner: #owner.clone(),
                            cases: vec![#(#cases),*],
                        },
                    )
                }
            }
            AnalysedType::Result(tr) => {
                let name = tr
                    .name
                    .as_ref()
                    .map(|n| {
                        let lit = Lit::Str(LitStr::new(n, Span::call_site()));
                        quote! { Some(#lit.to_string()) }
                    })
                    .unwrap_or_else(|| quote! { None });
                let owner = tr
                    .owner
                    .as_ref()
                    .map(|n| {
                        let lit = Lit::Str(LitStr::new(n, Span::call_site()));
                        quote! { Some(#lit.to_string()) }
                    })
                    .unwrap_or_else(|| quote! { None });
                let ok_type = tr.ok.as_ref().map(|t| self.analysed_type_as_value(t));
                let err_type = tr.err.as_ref().map(|t| self.analysed_type_as_value(t));
                let ok_tokens = match ok_type {
                    Some(t) => quote! { Some(Box::new(#t)) },
                    None => quote! { None },
                };
                let err_tokens = match err_type {
                    Some(t) => quote! { Some(Box::new(#t)) },
                    None => quote! { None },
                };
                quote! {
                    golem_wasm::analysis::AnalysedType::Result(
                        golem_wasm::analysis::TypeResult {
                            name: #name.clone(),
                            owner: #owner.clone(),
                            ok: #ok_tokens,
                            err: #err_tokens,
                        },
                    )
                }
            }
            AnalysedType::Option(to) => {
                let name = to
                    .name
                    .as_ref()
                    .map(|n| {
                        let lit = Lit::Str(LitStr::new(n, Span::call_site()));
                        quote! { Some(#lit.to_string()) }
                    })
                    .unwrap_or_else(|| quote! { None });
                let owner = to
                    .owner
                    .as_ref()
                    .map(|n| {
                        let lit = Lit::Str(LitStr::new(n, Span::call_site()));
                        quote! { Some(#lit.to_string()) }
                    })
                    .unwrap_or_else(|| quote! { None });
                let inner = self.analysed_type_as_value(&to.inner);
                quote! {
                    golem_wasm::analysis::AnalysedType::Option(
                        golem_wasm::analysis::TypeOption {
                            name: #name.clone(),
                            owner: #owner.clone(),
                            inner: Box::new(#inner),
                        },
                    )
                }
            }
            AnalysedType::Enum(te) => {
                let name = te
                    .name
                    .as_ref()
                    .map(|n| {
                        let lit = Lit::Str(LitStr::new(n, Span::call_site()));
                        quote! { Some(#lit.to_string()) }
                    })
                    .unwrap_or_else(|| quote! { None });
                let owner = te
                    .owner
                    .as_ref()
                    .map(|n| {
                        let lit = Lit::Str(LitStr::new(n, Span::call_site()));
                        quote! { Some(#lit.to_string()) }
                    })
                    .unwrap_or_else(|| quote! { None });
                let cases = &te.cases;
                quote! {
                    golem_wasm::analysis::AnalysedType::Enum(
                        golem_wasm::analysis::TypeEnum {
                            name: #name.clone(),
                            owner: #owner.clone(),
                            cases: vec![#(#cases.to_string()),*],
                        },
                    )
                }
            }
            AnalysedType::Flags(tf) => {
                let name = tf
                    .name
                    .as_ref()
                    .map(|n| {
                        let lit = Lit::Str(LitStr::new(n, Span::call_site()));
                        quote! { Some(#lit.to_string()) }
                    })
                    .unwrap_or_else(|| quote! { None });
                let owner = tf
                    .owner
                    .as_ref()
                    .map(|n| {
                        let lit = Lit::Str(LitStr::new(n, Span::call_site()));
                        quote! { Some(#lit.to_string()) }
                    })
                    .unwrap_or_else(|| quote! { None });
                let names = &tf.names;
                quote! {
                    golem_wasm::analysis::AnalysedType::Flags(
                        golem_wasm::analysis::TypeFlags {
                            name: #name.clone(),
                            owner: #owner.clone(),
                            names: vec![#(#names.to_string()),*],
                        },
                    )
                }
            }
            AnalysedType::Record(tr) => {
                let name = tr
                    .name
                    .as_ref()
                    .map(|n| {
                        let lit = Lit::Str(LitStr::new(n, Span::call_site()));
                        quote! { Some(#lit.to_string()) }
                    })
                    .unwrap_or_else(|| quote! { None });
                let owner = tr
                    .owner
                    .as_ref()
                    .map(|n| {
                        let lit = Lit::Str(LitStr::new(n, Span::call_site()));
                        quote! { Some(#lit.to_string()) }
                    })
                    .unwrap_or_else(|| quote! { None });
                let fields = tr
                    .fields
                    .iter()
                    .map(|field| {
                        let field_name = &field.name;
                        let field_type = self.analysed_type_as_value(&field.typ);
                        quote! {
                            golem_wasm::analysis::NameTypePair {
                                name: #field_name.to_string(),
                                typ: #field_type,
                            }
                        }
                    })
                    .collect::<Vec<_>>();
                quote! {
                    golem_wasm::analysis::AnalysedType::Record(
                        golem_wasm::analysis::TypeRecord {
                            name: #name.clone(),
                            owner: #owner.clone(),
                            fields: vec![#(#fields),*],
                        },
                    )
                }
            }
            AnalysedType::Tuple(tt) => {
                let name = tt
                    .name
                    .as_ref()
                    .map(|n| {
                        let lit = Lit::Str(LitStr::new(n, Span::call_site()));
                        quote! { Some(#lit.to_string()) }
                    })
                    .unwrap_or_else(|| quote! { None });
                let owner = tt
                    .owner
                    .as_ref()
                    .map(|n| {
                        let lit = Lit::Str(LitStr::new(n, Span::call_site()));
                        quote! { Some(#lit.to_string()) }
                    })
                    .unwrap_or_else(|| quote! { None });
                let items = tt
                    .items
                    .iter()
                    .map(|item| self.analysed_type_as_value(item))
                    .collect::<Vec<_>>();
                quote! {
                    golem_wasm::analysis::AnalysedType::Tuple(
                        golem_wasm::analysis::TypeTuple {
                            name: #name.clone(),
                            owner: #owner.clone(),
                            items: vec![#(#items),*],
                        },
                    )
                }
            }
            AnalysedType::List(tl) => {
                let name = tl
                    .name
                    .as_ref()
                    .map(|n| {
                        let lit = Lit::Str(LitStr::new(n, Span::call_site()));
                        quote! { Some(#lit.to_string()) }
                    })
                    .unwrap_or_else(|| quote! { None });
                let owner = tl
                    .owner
                    .as_ref()
                    .map(|n| {
                        let lit = Lit::Str(LitStr::new(n, Span::call_site()));
                        quote! { Some(#lit.to_string()) }
                    })
                    .unwrap_or_else(|| quote! { None });
                let inner = self.analysed_type_as_value(&tl.inner);
                quote! {
                    golem_wasm::analysis::AnalysedType::List(
                        golem_wasm::analysis::TypeList {
                            name: #name.clone(),
                            owner: #owner.clone(),
                            inner: Box::new(#inner),
                        },
                    )
                }
            }
            AnalysedType::Str(_) => {
                quote! {
                    golem_wasm::analysis::AnalysedType::Str(
                        golem_wasm::analysis::TypeStr,
                    )
                }
            }
            AnalysedType::Chr(_) => {
                quote! {
                    golem_wasm::analysis::AnalysedType::Chr(
                        golem_wasm::analysis::TypeChr,
                    )
                }
            }
            AnalysedType::F64(_) => {
                quote! {
                    golem_wasm::analysis::AnalysedType::F64(
                        golem_wasm::analysis::TypeF64,
                    )
                }
            }
            AnalysedType::F32(_) => {
                quote! {
                    golem_wasm::analysis::AnalysedType::F32(
                        golem_wasm::analysis::TypeF32,
                    )
                }
            }
            AnalysedType::U64(_) => {
                quote! {
                    golem_wasm::analysis::AnalysedType::U64(
                        golem_wasm::analysis::TypeU64,
                    )
                }
            }
            AnalysedType::S64(_) => {
                quote! {
                    golem_wasm::analysis::AnalysedType::S64(
                        golem_wasm::analysis::TypeS64,
                    )
                }
            }
            AnalysedType::U32(_) => {
                quote! {
                    golem_wasm::analysis::AnalysedType::U32(
                        golem_wasm::analysis::TypeU32,
                    )
                }
            }
            AnalysedType::S32(_) => {
                quote! {
                    golem_wasm::analysis::AnalysedType::S32(
                        golem_wasm::analysis::TypeS32,
                    )
                }
            }
            AnalysedType::U16(_) => {
                quote! {
                    golem_wasm::analysis::AnalysedType::U16(
                        golem_wasm::analysis::TypeU16,
                    )
                }
            }
            AnalysedType::S16(_) => {
                quote! {
                    golem_wasm::analysis::AnalysedType::S16(
                        golem_wasm::analysis::TypeS16,
                    )
                }
            }
            AnalysedType::U8(_) => {
                quote! {
                    golem_wasm::analysis::AnalysedType::U8(
                        golem_wasm::analysis::TypeU8,
                    )
                }
            }
            AnalysedType::S8(_) => {
                quote! {
                    golem_wasm::analysis::AnalysedType::S8(
                        golem_wasm::analysis::TypeS8,
                    )
                }
            }
            AnalysedType::Bool(_) => {
                quote! {
                    golem_wasm::analysis::AnalysedType::Bool(
                        golem_wasm::analysis::TypeBool,
                    )
                }
            }
            AnalysedType::Handle(th) => {
                let name = th
                    .name
                    .as_ref()
                    .map(|n| {
                        let lit = Lit::Str(LitStr::new(n, Span::call_site()));
                        quote! { Some(#lit.to_string()) }
                    })
                    .unwrap_or_else(|| quote! { None });
                let owner = th
                    .owner
                    .as_ref()
                    .map(|n| {
                        let lit = Lit::Str(LitStr::new(n, Span::call_site()));
                        quote! { Some(#lit.to_string()) }
                    })
                    .unwrap_or_else(|| quote! { None });
                let resource_id = th.resource_id.0;
                let mode = match th.mode {
                    golem_wasm::analysis::AnalysedResourceMode::Owned => {
                        quote! { golem_wasm::analysis::AnalysedResourceMode::Owned }
                    }
                    golem_wasm::analysis::AnalysedResourceMode::Borrowed => {
                        quote! { golem_wasm::analysis::AnalysedResourceMode::Borrowed }
                    }
                };
                quote! {
                    golem_wasm::analysis::AnalysedType::Handle(
                        golem_wasm::analysis::TypeHandle {
                            name: #name.clone(),
                            owner: #owner.clone(),
                            resource_id: golem_wasm::analysis::AnalysedResourceId(#resource_id),
                            mode: #mode,
                        },
                    )
                }
            }
        }
    }

    fn decode_from_data_value(
        &mut self,
        ident: &Ident,
        data_schema: &DataSchema,
    ) -> anyhow::Result<TokenStream> {
        match data_schema {
            DataSchema::Tuple(elements) => {
                if elements.elements.len() > 1 {
                    Err(anyhow!("multiple result values not supported"))
                } else if elements.elements.is_empty() {
                    Ok(quote! {
                        Ok(())
                    })
                } else {
                    let element = &elements.elements[0];
                    match &element.schema {
                        ElementSchema::ComponentModel(_) => {
                            if let Some(return_type) =
                                self.return_type_from_data_schema(data_schema)?
                            {
                                Ok(quote! {
                                    match #ident {
                                        golem_common::model::agent::DataValue::Tuple(element_values) => {
                                            match element_values.elements.get(0) {
                                                Some(golem_common::model::agent::ElementValue::ComponentModel(golem_common::model::agent::ComponentModelElementValue { value: vnt })) => {
                                                    Ok(Some(<#return_type>::from_value_and_type(vnt.clone()).map_err(
                                                        |err| golem_client::bridge::ClientError::InvocationFailed {
                                                            message: format!("Failed to decode result value: {err}"),
                                                        },
                                                    )?))
                                                }
                                                _ => Err(golem_client::bridge::ClientError::InvocationFailed {
                                                    message: format!("Failed to decode result value"),
                                                })?,
                                            }
                                        }
                                        _ => Err(golem_client::bridge::ClientError::InvocationFailed {
                                            message: format!("Failed to decode result value"),
                                        })?,
                                    }
                                })
                            } else {
                                Ok(quote! { Ok(()) })
                            }
                        }
                        ElementSchema::UnstructuredText(descriptor) => {
                            let unstructured_text = match &descriptor.restrictions {
                                Some(restrictions) => {
                                    let languages_enum = self.get_languages_enum(restrictions);
                                    quote! {
                                        golem_wasm::agentic::unstructured_text::UnstructuredText<#languages_enum>
                                    }
                                }
                                None => {
                                    quote! { golem_wasm::agentic::unstructured_text::UnstructuredText }
                                }
                            };
                            Ok(quote! {
                                match #ident {
                                    golem_common::model::agent::DataValue::Tuple(element_values) => {
                                        match element_values.elements.get(0) {
                                            Some(golem_common::model::agent::ElementValue::UnstructuredText(golem_common::model::agent::UnstructuredTextElementValue { value: text_ref, .. })) => {
                                                <#unstructured_text>::from_text_reference(text_ref.clone())
                                                    .map(Some)
                                                    .map_err(|err| golem_client::bridge::ClientError::InvocationFailed {
                                                        message: format!("Failed to decode result value: {err}"),
                                                    })
                                            }
                                            _ => Err(golem_client::bridge::ClientError::InvocationFailed {
                                                message: format!("Failed to decode result value"),
                                            })?,
                                        }
                                    }
                                    _ => Err(golem_client::bridge::ClientError::InvocationFailed {
                                        message: format!("Failed to decode result value"),
                                    })?,
                                }
                            })
                        }
                        ElementSchema::UnstructuredBinary(descriptor) => {
                            let unstructured_binary = match &descriptor.restrictions {
                                Some(restrictions) => {
                                    let mimetypes_enum = self.get_mimetypes_enum(restrictions);
                                    quote! {
                                        golem_wasm::agentic::unstructured_binary::UnstructuredBinary<#mimetypes_enum>
                                    }
                                }
                                None => {
                                    quote! { golem_wasm::agentic::unstructured_binary::UnstructuredBinary<String> }
                                }
                            };
                            Ok(quote! {
                                match #ident {
                                    golem_common::model::agent::DataValue::Tuple(element_values) => {
                                        match element_values.elements.get(0) {
                                            Some(golem_common::model::agent::ElementValue::UnstructuredBinary(golem_common::model::agent::UnstructuredBinaryElementValue { value: binary_ref, .. })) => {
                                                <#unstructured_binary>::from_binary_reference(binary_ref.clone())
                                                    .map(Some)
                                                    .map_err(|err| golem_client::bridge::ClientError::InvocationFailed {
                                                        message: format!("Failed to decode result value: {err}"),
                                                    })
                                            }
                                            _ => Err(golem_client::bridge::ClientError::InvocationFailed {
                                                message: format!("Failed to decode result value"),
                                            })?,
                                        }
                                    }
                                    _ => Err(golem_client::bridge::ClientError::InvocationFailed {
                                        message: format!("Failed to decode result value"),
                                    })?,
                                }
                            })
                        }
                    }
                }
            }
            DataSchema::Multimodal(elements) => {
                let name = self.get_or_create_multimodal(elements);
                let name = Ident::new(&name, Span::call_site());
                Ok(quote! {
                    match #ident {
                        golem_common::model::agent::DataValue::Multimodal(multimodal_value) => {
                            Ok(Some(crate::multimodal::Multimodal::<crate::multimodal::#name>::from_named_element_values(multimodal_value)?))
                        }
                        _ => Err(golem_client::bridge::ClientError::InvocationFailed {
                            message: format!("Failed to decode result value"),
                        })
                    }
                })
            }
        }
    }

    fn global_config(&self) -> TokenStream {
        quote! {
            #[cfg(feature = "client")]
            static CONFIG: std::sync::OnceLock<golem_client::bridge::Configuration> = std::sync::OnceLock::new();

            #[cfg(feature = "client")]
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

    fn multimodals(&mut self) -> anyhow::Result<TokenStream> {
        if self.known_multimodals.is_empty() {
            Ok(quote! {})
        } else {
            let mut multimodal_enums = Vec::new();

            for (named_elements, name) in self.known_multimodals.clone() {
                let name = Ident::new(&name, Span::call_site());
                let mut cases = Vec::new();
                let mut to_named_element_value_cases = Vec::new();
                let mut from_named_element_value_cases = Vec::new();

                for named_element in &named_elements.elements {
                    let case_name_lit =
                        Lit::Str(LitStr::new(&named_element.name, Span::call_site()));
                    let case_name = Ident::new(
                        &self.to_rust_case_name(&named_element.name),
                        Span::call_site(),
                    );
                    let case_type = self.element_schema_to_typeref(&named_element.schema)?;
                    cases.push(quote! { #case_name(#case_type) });

                    let encode_value = match &named_element.schema {
                        ElementSchema::ComponentModel(_) => {
                            quote! { golem_common::model::agent::ElementValue::ComponentModel(golem_common::model::agent::ComponentModelElementValue { value: value.clone().into_value_and_type() }) }
                        }
                        ElementSchema::UnstructuredText(_) => {
                            quote! { golem_common::model::agent::ElementValue::UnstructuredText(golem_common::model::agent::UnstructuredTextElementValue { value: value.clone().into_text_reference(), descriptor: golem_common::model::agent::TextDescriptor::default() }) }
                        }
                        ElementSchema::UnstructuredBinary(_) => {
                            quote! { golem_common::model::agent::ElementValue::UnstructuredBinary(golem_common::model::agent::UnstructuredBinaryElementValue { value: value.clone().into_binary_reference(), descriptor: golem_common::model::agent::BinaryDescriptor::default() }) }
                        }
                    };
                    to_named_element_value_cases.push(quote! {
                        Self::#case_name(value) => golem_common::model::agent::NamedElementValue {
                            name: #case_name_lit.to_string(),
                            value: #encode_value,
                            schema_index: 0,
                        },
                    });

                    let decode_value = match &named_element.schema {
                        ElementSchema::ComponentModel(schema) => {
                            let value_type = self.wit_type_to_typeref(&schema.element_type)?;
                            quote! {
                                let value = match &named_element_value.value {
                                    golem_common::model::agent::ElementValue::ComponentModel(golem_common::model::agent::ComponentModelElementValue { value: vnt }) => {
                                        Ok(<#value_type>::from_value_and_type(vnt.clone()).map_err(
                                            |err| golem_client::bridge::ClientError::InvocationFailed {
                                                message: format!("Failed to decode result value: {err}"),
                                            }
                                        )?)
                                    }
                                    _ => {
                                        Err(golem_client::bridge::ClientError::InvocationFailed {
                                            message: format!("Failed to decode result value"),
                                        })
                                    }
                                }?;
                            }
                        }
                        ElementSchema::UnstructuredText(descriptor) => {
                            let unstructured_text = match &descriptor.restrictions {
                                Some(restrictions) => {
                                    let languages_enum = self.get_languages_enum(restrictions);
                                    quote! {
                                        golem_wasm::agentic::unstructured_text::UnstructuredText<#languages_enum>
                                    }
                                }
                                None => {
                                    quote! { golem_wasm::agentic::unstructured_text::UnstructuredText }
                                }
                            };
                            quote! {
                                let value = match &named_element_value.value {
                                    golem_common::model::agent::ElementValue::UnstructuredText(golem_common::model::agent::UnstructuredTextElementValue { value: text_ref, .. }) => {
                                        <#unstructured_text>::from_text_reference(text_ref.clone())
                                            .map_err(|err| golem_client::bridge::ClientError::InvocationFailed {
                                                message: format!("Failed to decode result value: {err}"),
                                            })
                                    }
                                    _ => {
                                        Err(golem_client::bridge::ClientError::InvocationFailed {
                                            message: format!("Failed to decode result value"),
                                        })
                                    }
                                }?;
                            }
                        }
                        ElementSchema::UnstructuredBinary(descriptor) => {
                            let unstructured_binary = match &descriptor.restrictions {
                                Some(restrictions) => {
                                    let mimetypes_enum = self.get_mimetypes_enum(restrictions);
                                    quote! {
                                        golem_wasm::agentic::unstructured_binary::UnstructuredBinary<#mimetypes_enum>
                                    }
                                }
                                None => {
                                    quote! { golem_wasm::agentic::unstructured_binary::UnstructuredBinary<String> }
                                }
                            };
                            quote! {
                                let value = match &named_element_value.value {
                                    golem_common::model::agent::ElementValue::UnstructuredBinary(golem_common::model::agent::UnstructuredBinaryElementValue { value: binary_ref, .. }) => {
                                        <#unstructured_binary>::from_binary_reference(binary_ref.clone())
                                            .map_err(|err| golem_client::bridge::ClientError::InvocationFailed {
                                                message: format!("Failed to decode result value: {err}"),
                                            })
                                    }
                                    _ => {
                                        Err(golem_client::bridge::ClientError::InvocationFailed {
                                            message: format!("Failed to decode result value"),
                                        })
                                    }
                                }?;
                            }
                        }
                    };
                    from_named_element_value_cases.push(quote! {
                        #case_name_lit => {
                            #decode_value
                            values.push(Self::#case_name(value));
                        },
                    });
                }

                multimodal_enums.push(quote! {
                    pub enum #name {
                        #(#cases),*
                    }

                    impl MultimodalEnum for #name {
                        fn to_named_element_value(&self) -> golem_common::model::agent::NamedElementValue {
                            match self {
                                #(#to_named_element_value_cases)*
                            }
                        }

                        fn from_named_element_values(named_element_values: golem_common::model::agent::NamedElementValues) -> Result<Multimodal<Self>, golem_client::bridge::ClientError> {
                            let mut values = Vec::new();
                            for named_element_value in named_element_values.elements {
                                match named_element_value.name.as_str() {
                                    #(#from_named_element_value_cases)*
                                    _ => {
                                        return Err(golem_client::bridge::ClientError::InvocationFailed {
                                            message: format!("Unknown multimodal element name: {}", named_element_value.name),
                                        });
                                    }
                                }
                            }
                            Ok(Multimodal {
                                values: nonempty_collections::NEVec::try_from_vec(values).ok_or_else(|| golem_client::bridge::ClientError::InvocationFailed { message: "Empty multimodal value".to_string() })?,
                            })
                        }
                    }
                });
            }

            Ok(quote! {
                pub mod multimodal {
                    use super::*;
                    use golem_common::base_model::agent::{UnstructuredBinaryExtensions, UnstructuredTextExtensions};
                    use golem_wasm::{FromValueAndType, IntoValueAndType};

                    #[derive(Debug, Clone)]
                    pub struct Multimodal<T: MultimodalEnum> {
                        pub values: nonempty_collections::NEVec<T>
                    }

                    impl<T: MultimodalEnum> Multimodal<T> {
                        pub fn from_named_element_values(named_element_values: golem_common::model::agent::NamedElementValues) -> Result<Self, golem_client::bridge::ClientError> {
                            T::from_named_element_values(named_element_values)
                        }
                    }

                    pub trait MultimodalEnum: Sized {
                        fn to_named_element_value(&self) -> golem_common::model::agent::NamedElementValue;
                        fn from_named_element_values(named_element_values: golem_common::model::agent::NamedElementValues) -> Result<Multimodal<Self>, golem_client::bridge::ClientError>;
                    }

                    #(#multimodal_enums)*
                }
            })
        }
    }

    fn languages_module(&self) -> TokenStream {
        if self.generated_language_enums.is_empty() {
            quote! {}
        } else {
            let mut language_enums = Vec::new();

            for (types, name) in &self.generated_language_enums {
                let ident = Ident::new(name, Span::call_site());

                let mut cases = Vec::new();
                let mut code_strings = Vec::new();
                let mut from_match_cases = Vec::new();
                let mut to_match_cases = Vec::new();

                for typ in types {
                    let ident = Ident::new(
                        &to_rust_ident(&typ.language_code, false).to_upper_camel_case(),
                        Span::call_site(),
                    );
                    let lit = Lit::Str(LitStr::new(&typ.language_code, Span::call_site()));

                    cases.push(quote! { #ident });
                    code_strings.push(quote! { #lit });
                    from_match_cases.push(quote! { #lit => Some(Self::#ident) });
                    to_match_cases.push(quote! { Self::#ident => #lit });
                }
                from_match_cases.push(quote! { _ => None });

                language_enums.push(quote! {
                    #[derive(Debug, Clone)]
                    pub enum #ident {
                        #(#cases),*
                    }

                    impl golem_wasm::agentic::unstructured_text::AllowedLanguages for #ident {
                        fn all() -> &'static [&'static str] {
                            &[#(#code_strings),*]
                        }

                        fn from_language_code(code: &str) -> Option<Self>
                        where
                            Self: Sized {
                            match code {
                                #(#from_match_cases),*
                            }
                        }

                        fn to_language_code(&self) -> &'static str {
                            match self {
                                #(#to_match_cases),*
                            }
                        }
                    }
                });
            }

            quote! {
                pub mod languages {
                    #(#language_enums)*
                }
            }
        }
    }

    fn mimetypes_module(&self) -> TokenStream {
        if self.generated_mimetypes_enums.is_empty() {
            quote! {}
        } else {
            let mut mimetypes_enums = Vec::new();

            for (types, name) in &self.generated_mimetypes_enums {
                let ident = Ident::new(name, Span::call_site());

                let mut cases = Vec::new();
                let mut code_strings = Vec::new();
                let mut from_match_cases = Vec::new();
                let mut to_match_cases = Vec::new();

                for typ in types {
                    let enum_variant_ident = Ident::new(
                        &to_rust_ident(&typ.mime_type, false).to_upper_camel_case(),
                        Span::call_site(),
                    );
                    let lit = Lit::Str(LitStr::new(&typ.mime_type, Span::call_site()));

                    cases.push(quote! { #enum_variant_ident });
                    code_strings.push(quote! { #lit });
                    from_match_cases.push(quote! { #lit => Some(Self::#enum_variant_ident), });
                    to_match_cases.push(quote! { Self::#enum_variant_ident => #lit.to_string(), });
                }

                mimetypes_enums.push(quote! {
                    #[derive(Debug, Clone)]
                    pub enum #ident {
                        #(#cases),*
                    }

                    impl golem_wasm::agentic::unstructured_binary::AllowedMimeTypes for #ident {
                        fn all() -> &'static [&'static str] {
                            &[#(#code_strings),*]
                        }

                        fn from_string(mime_type: &str) -> Option<Self>
                        where
                            Self: Sized {
                            match mime_type {
                                #(#from_match_cases)*
                                _ => None,
                            }
                        }

                        fn to_string(&self) -> String {
                            match self {
                                #(#to_match_cases)*
                            }
                        }
                    }
                });
            }

            quote! {
                pub mod mimetypes {
                    #(#mimetypes_enums)*
                }
            }
        }
    }

    fn type_definitions(&mut self) -> anyhow::Result<TokenStream> {
        let mut type_definitions = Vec::new();

        for (typ, name) in self.type_naming.types() {
            let def = self.wit_type_to_typedef(name, typ)?;
            type_definitions.push(def);
        }

        Ok(quote! {
            #(#type_definitions)*
        })
    }

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

    fn package_name(&self) -> String {
        self.package_crate_name().to_snake_case()
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

    fn optional_dep_item(&self, crate_path: &str, features: &[&str]) -> anyhow::Result<Item> {
        let mut item = self.dep_item(crate_path, features)?;
        item["optional"] = value(true);
        Ok(item)
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
