// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use heck::{ToKebabCase, ToLowerCamelCase, ToShoutySnakeCase, ToSnakeCase, ToUpperCamelCase};
use proc_macro::TokenStream;
use std::collections::HashSet;
use syn::ext::IdentExt;
use syn::{DeriveInput, Fields, Ident, LitStr, parse_macro_input};

struct ConfigField<'a> {
    ident: &'a Ident,
    ty: &'a syn::Type,
    name: String,
    secret: bool,
    nested: bool,
    default: Option<LitStr>,
}

fn config_fields(ast: &DeriveInput) -> syn::Result<Vec<ConfigField<'_>>> {
    let mut rename_all = "camelCase".to_string();
    for attr in &ast.attrs {
        if attr.path().is_ident("config_schema") {
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("rename_all") {
                    rename_all = meta.value()?.parse::<LitStr>()?.value();
                    Ok(())
                } else {
                    Err(meta.error("expected rename_all"))
                }
            })?;
        }
    }
    let rename: fn(&str) -> String = match rename_all.as_str() {
        "preserve" => str::to_string,
        "camelCase" => str::to_lower_camel_case,
        "PascalCase" => str::to_upper_camel_case,
        "snake_case" => str::to_snake_case,
        "kebab-case" => str::to_kebab_case,
        "SCREAMING_SNAKE_CASE" => str::to_shouty_snake_case,
        _ => {
            return Err(syn::Error::new_spanned(
                ast,
                "unsupported config rename_all strategy",
            ));
        }
    };
    let fields = match &ast.data {
        syn::Data::Struct(data) => match &data.fields {
            Fields::Named(fields) => &fields.named,
            _ => {
                return Err(syn::Error::new_spanned(
                    ast,
                    "ConfigSchema only supports structs with named fields",
                ));
            }
        },
        _ => {
            return Err(syn::Error::new_spanned(
                ast,
                "ConfigSchema can only be derived for structs",
            ));
        }
    };
    let mut names = HashSet::new();
    let mut result = Vec::new();
    for field in fields {
        let ident = field.ident.as_ref().unwrap();
        let mut config = ConfigField {
            ident,
            ty: &field.ty,
            name: rename(&ident.unraw().to_string()),
            secret: false,
            nested: false,
            default: None,
        };
        for attr in &field.attrs {
            if attr.path().is_ident("config_schema") {
                attr.parse_nested_meta(|meta| {
                    if meta.path.is_ident("secret") {
                        config.secret = true;
                    } else if meta.path.is_ident("nested") {
                        config.nested = true;
                    } else if meta.path.is_ident("rename") {
                        config.name = meta.value()?.parse::<LitStr>()?.value();
                    } else if meta.path.is_ident("default") {
                        config.default = Some(meta.value()?.parse::<LitStr>()?);
                    } else {
                        return Err(meta.error(
                            "expected secret, nested, rename or default (canonical JSON string)",
                        ));
                    }
                    Ok(())
                })?;
            }
        }
        if config.name.is_empty() || config.name.contains('.') || config.name.starts_with('$') {
            return Err(syn::Error::new_spanned(
                field,
                "config fields must have nonempty names without '.' or a leading '$'",
            ));
        }
        if !names.insert(config.name.clone()) {
            return Err(syn::Error::new_spanned(
                field,
                "duplicate canonical config field name",
            ));
        }
        if config.secret && config.nested
            || config.default.is_some() && (config.secret || config.nested)
        {
            return Err(syn::Error::new_spanned(
                field,
                "secret and nested fields cannot have defaults or combine their kinds",
            ));
        }
        result.push(config);
    }
    Ok(result)
}

pub fn derive_config_schema(input: TokenStream, golem_rust_crate_ident: &Ident) -> TokenStream {
    let ast = parse_macro_input!(input as DeriveInput);

    let fields = match config_fields(&ast) {
        Ok(fields) => fields,
        Err(error) => return error.to_compile_error().into(),
    };

    let struct_name = &ast.ident;
    let rpc_struct_name = Ident::new(&format!("{struct_name}Rpc"), struct_name.span());

    let config_schema_impl = generate_config_schema_impl(
        struct_name,
        &rpc_struct_name,
        golem_rust_crate_ident,
        &fields,
    );
    let rpc_struct = generate_rpc_struct(&rpc_struct_name, golem_rust_crate_ident, &fields);
    let as_rpc_impl =
        generate_into_rpc_config_param_impl(&rpc_struct_name, golem_rust_crate_ident, &fields);

    let expanded = quote::quote! {
        #rpc_struct
        #as_rpc_impl
        #config_schema_impl
    };

    expanded.into()
}

fn generate_config_schema_impl(
    struct_name: &Ident,
    rpc_struct_name: &Ident,
    golem_rust_crate_ident: &Ident,
    fields: &[ConfigField<'_>],
) -> proc_macro2::TokenStream {
    let mut append_config_entries = Vec::new();
    let mut append_wire_entries = Vec::new();
    let mut load_entries = Vec::new();

    for field in fields {
        let field_ident = field.ident;
        let field_name_str = &field.name;
        let field_ty = field.ty;
        let default_value = match &field.default {
            Some(json) => quote::quote! {
                Some(#golem_rust_crate_ident::agentic::static_config_default::<#field_ty>(#json)
                    .expect(concat!("invalid code default for config field ", #field_name_str)))
            },
            None => quote::quote! { None },
        };

        let wire_entry = if field.nested {
            quote::quote! {
                config_entries.extend(<#field_ty as #golem_rust_crate_ident::agentic::ConfigSchema>::describe_wire_config(&field_path, builder));
            }
        } else {
            let (source, value_type) = if field.secret {
                (
                    quote::quote! { Secret },
                    quote::quote! {{
                        let inner = <<#field_ty as #golem_rust_crate_ident::agentic::InnerTypeHelper>::Type as #golem_rust_crate_ident::WireSchema>::append_schema(builder);
                        builder.push(#golem_rust_crate_ident::schema::wit::wire::SchemaTypeBody::SecretType(
                            #golem_rust_crate_ident::schema::wit::wire::SecretSpec { inner, category: None }
                        ))
                    }},
                )
            } else {
                (
                    quote::quote! { Local },
                    quote::quote! { <#field_ty as #golem_rust_crate_ident::WireSchema>::append_schema(builder) },
                )
            };
            quote::quote! {
                config_entries.push(#golem_rust_crate_ident::golem_agentic::golem::agent::common::AgentConfigDeclaration {
                    source: #golem_rust_crate_ident::golem_agentic::golem::agent::common::AgentConfigSource::#source,
                    path: field_path,
                    value_type: #value_type,
                    default_value: {
                        let default_value: Option<#golem_rust_crate_ident::schema::SchemaValue> = #default_value;
                        default_value.as_ref().map(|value| {
                            #golem_rust_crate_ident::schema::wit::encode_value(value)
                                .expect("invalid static config default")
                        })
                    },
                });
            }
        };
        append_wire_entries.push(quote::quote! {{
            let mut field_path = path.to_vec();
            field_path.push(#field_name_str.to_string());
            #wire_entry
        }});

        if field.nested {
            append_config_entries.push(quote::quote! {
                {
                    let mut field_path = path.to_vec();
                    field_path.push(#field_name_str.to_string());
                    let mut nested_entries = <#field_ty as #golem_rust_crate_ident::agentic::ConfigSchema>::describe_config(&field_path);
                    config_entries.append(&mut nested_entries);
                }
            });
            load_entries.push(quote::quote! {
                #field_ident: {
                    let mut field_path = path.to_vec();
                    field_path.push(#field_name_str.to_string());
                    <#field_ty as #golem_rust_crate_ident::agentic::ConfigSchema>::load(&field_path)?
                }
            });
        } else if field.secret {
            append_config_entries.push(quote::quote! {
                {
                    let mut field_path = path.to_vec();
                    field_path.push(#field_name_str.to_string());
                    let config_entry = #golem_rust_crate_ident::agentic::ExtendedAgentConfigDeclaration {
                        source: #golem_rust_crate_ident::golem_agentic::golem::agent::common::AgentConfigSource::Secret,
                        path: field_path,
                        value_type: #golem_rust_crate_ident::agentic::secret_schema_graph::<<#field_ty as #golem_rust_crate_ident::agentic::InnerTypeHelper>::Type>()
                            .expect("failed to build config schema graph"),
                        default_value: None,
                    };
                    config_entries.push(config_entry);
                }
            });
            load_entries.push(quote::quote! {
                #field_ident: {
                    let mut field_path = path.to_vec();
                    field_path.push(#field_name_str.to_string());
                    #golem_rust_crate_ident::agentic::Secret::new(field_path)
                }
            });
        } else {
            append_config_entries.push(quote::quote! {
                {
                    let mut field_path = path.to_vec();
                    field_path.push(#field_name_str.to_string());
                    let config_entry = #golem_rust_crate_ident::agentic::ExtendedAgentConfigDeclaration {
                        source: #golem_rust_crate_ident::golem_agentic::golem::agent::common::AgentConfigSource::Local,
                        path: field_path,
                        value_type: #golem_rust_crate_ident::schema::try_into_schema_graph::<#field_ty>()
                            .expect("failed to build config schema graph"),
                        default_value: #default_value,
                    };
                    config_entries.push(config_entry);
                }
            });
            load_entries.push(quote::quote! {
                #field_ident: {
                    let mut field_path = path.to_vec();
                    field_path.push(#field_name_str.to_string());
                    let graph = #golem_rust_crate_ident::schema::wit::direct::schema::<#field_ty>();
                    let value = #golem_rust_crate_ident::golem_agentic::golem::agent::host::get_config_value(
                        &field_path,
                        &graph,
                    )?;
                    #golem_rust_crate_ident::schema::wit::direct::decode::<#field_ty>(value)
                        .expect("failed deserializing config value")
                }
            });
        }
    }

    quote::quote! {
        impl #golem_rust_crate_ident::agentic::ConfigSchema for #struct_name {
            type RpcType = #rpc_struct_name;

            fn describe_config(path: &[String]) -> Vec<#golem_rust_crate_ident::agentic::ExtendedAgentConfigDeclaration> {
                let mut config_entries = Vec::new();
                #(#append_config_entries)*
                config_entries
            }

            fn describe_wire_config(path: &[String], builder: &mut #golem_rust_crate_ident::schema::wit::direct::WireSchemaBuilder)
                -> Vec<#golem_rust_crate_ident::golem_agentic::golem::agent::common::AgentConfigDeclaration> {
                let mut config_entries = Vec::new();
                #(#append_wire_entries)*
                config_entries
            }

            fn load(path: &[String]) -> Result<Self, #golem_rust_crate_ident::agentic::ConfigValueError> {
                Ok(Self {
                    #(#load_entries),*
                })
            }
        }
    }
}

fn generate_rpc_struct(
    rpc_struct_name: &Ident,
    golem_rust_crate_ident: &Ident,
    fields: &[ConfigField<'_>],
) -> proc_macro2::TokenStream {
    let mut rpc_fields = Vec::new();

    for field in fields {
        let field_ident = field.ident;
        let field_ty = field.ty;

        if field.secret {
            continue; // secrets are omitted
        } else if field.nested {
            rpc_fields.push(quote::quote! {
                pub #field_ident: <#field_ty as #golem_rust_crate_ident::agentic::ConfigSchema>::RpcType
            });
        } else {
            rpc_fields.push(quote::quote! {
                pub #field_ident: ::std::option::Option<#field_ty>
            });
        }
    }

    quote::quote! {
        #[derive(Default)]
        pub struct #rpc_struct_name {
            #(#rpc_fields),*
        }
    }
}

fn generate_into_rpc_config_param_impl(
    rpc_struct_name: &Ident,
    golem_rust_crate_ident: &Ident,
    fields: &[ConfigField<'_>],
) -> proc_macro2::TokenStream {
    let mut field_processes = Vec::new();

    for field in fields {
        let field_ident = field.ident;
        let field_name_str = &field.name;
        let field_ty = field.ty;

        if field.secret {
            continue; // secrets omitted
        } else if field.nested {
            field_processes.push(quote::quote! {
                {
                    let mut field_path = path.to_vec();
                    field_path.push(#field_name_str.to_string());

                    let mut nested_fields_as_params = #golem_rust_crate_ident::agentic::IntoRpcConfigParam::into_rpc_param(self.#field_ident, &field_path);
                    result.append(&mut nested_fields_as_params);
                }
            });
        } else {
            field_processes.push(quote::quote! {
                {
                    if let Some(value) = self.#field_ident {
                        let mut field_path = path.to_vec();
                        field_path.push(#field_name_str.to_string());

                        result.push(#golem_rust_crate_ident::golem_agentic::golem::agent::common::TypedAgentConfigValue {
                            path: field_path,
                            value: #golem_rust_crate_ident::schema::wit::wire::TypedSchemaValue {
                                graph: #golem_rust_crate_ident::schema::wit::direct::schema::<#field_ty>(),
                                value: #golem_rust_crate_ident::schema::wit::direct::encode(&value)
                                    .expect("failed to encode config value"),
                            },
                        });
                    }
                }
            });
        }
    }

    quote::quote! {
        impl #golem_rust_crate_ident::agentic::IntoRpcConfigParam for #rpc_struct_name {
            fn into_rpc_param(self, path: &[String]) -> Vec<#golem_rust_crate_ident::golem_agentic::golem::agent::common::TypedAgentConfigValue> {
                let mut result = Vec::new();
                #(#field_processes)*
                result
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::config_fields;
    use test_r::test;

    #[test]
    fn config_schema_rejects_invalid_static_declarations() {
        for (source, diagnostic) in [
            (
                r#"struct Config { #[config_schema(default = factory)] value: u32 }"#,
                "expected string literal",
            ),
            (
                r#"struct Config { #[config_schema(secret, default = "null")] value: Secret<String> }"#,
                "cannot have defaults",
            ),
            (
                r#"struct Config { #[config_schema(nested, default = "{}")] value: Nested }"#,
                "cannot have defaults",
            ),
            (
                r#"struct Config { #[config_schema(rename = "$reserved")] value: u32 }"#,
                "leading '$'",
            ),
            (
                r#"struct Config { #[config_schema(rename = "a.b")] value: u32 }"#,
                "without '.'",
            ),
            (
                r#"#[config_schema(rename_all = "camelCase")] struct Config { max_chars: u32, maxChars: u32 }"#,
                "duplicate canonical",
            ),
            (
                r#"#[config_schema(rename_all = "unknown")] struct Config {}"#,
                "unsupported config rename_all",
            ),
        ] {
            let ast = syn::parse_str(source).unwrap();
            let error = config_fields(&ast)
                .err()
                .expect("declaration must be rejected");
            assert!(error.to_string().contains(diagnostic), "{source}: {error}");
        }
    }
}
