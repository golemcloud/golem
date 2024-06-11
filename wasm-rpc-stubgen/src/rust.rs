// Copyright 2024 Golem Cloud
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

use crate::stub::{FunctionResultStub, FunctionStub, StubDefinition};
use anyhow::anyhow;
use heck::{ToShoutySnakeCase, ToSnakeCase, ToUpperCamelCase};
use proc_macro2::{Ident, Span, TokenStream};
use quote::quote;
use std::fs;
use wit_bindgen_rust::to_rust_ident;
use wit_parser::{
    Enum, Flags, Handle, Record, Resolve, Result_, Tuple, Type, TypeDefKind, TypeId, TypeOwner,
    Variant,
};

pub fn generate_stub_source(def: &StubDefinition) -> anyhow::Result<()> {
    let root_ns = Ident::new(
        &def.root_package_name.namespace.to_snake_case(),
        Span::call_site(),
    );
    let root_name = Ident::new(
        &format!("{}_stub", def.root_package_name.name.to_snake_case()),
        Span::call_site(),
    );

    let mut struct_defs = Vec::new();
    for interface in &def.interfaces {
        let interface_ident = to_rust_ident(&interface.name).to_upper_camel_case();
        let interface_name = Ident::new(&interface_ident, Span::call_site());

        let additional_fields = if interface.is_resource() {
            vec![quote! {
                id: u64,
                uri: golem_wasm_rpc::Uri
            }]
        } else {
            vec![]
        };
        let struct_fns: Vec<TokenStream> = if interface.is_resource() {
            vec![quote! {
                pub fn from_remote_handle(uri: golem_wasm_rpc::Uri, id: u64) -> Self {
                    Self {
                        rpc: WasmRpc::new(&uri),
                        id,
                        uri,
                    }
                }
            }]
        } else {
            vec![]
        };

        struct_defs.push(quote! {
           pub struct #interface_name {
                rpc: WasmRpc,
                #(#additional_fields),*
           }

           impl #interface_name {
             #(#struct_fns)*
           }
        });
    }

    let mut interface_impls = Vec::new();
    for interface in &def.interfaces {
        let interface_ident = to_rust_ident(&interface.name).to_upper_camel_case();
        let interface_name = Ident::new(&interface_ident, Span::call_site());
        let guest_interface_name =
            Ident::new(&format!("Guest{}", interface_ident), Span::call_site());

        let mut fn_impls = Vec::new();
        for function in &interface.functions {
            fn_impls.push(generate_function_stub_source(
                def,
                function,
                if interface.global {
                    None
                } else {
                    match &interface.owner_interface {
                        Some(owner) => Some(format!("{owner}/{}", &interface.name)),
                        None => Some(interface.name.clone()),
                    }
                },
                if interface.is_resource() {
                    FunctionMode::Method
                } else {
                    FunctionMode::Global
                },
            )?);
        }

        for function in &interface.static_functions {
            fn_impls.push(generate_function_stub_source(
                def,
                function,
                if interface.global {
                    None
                } else {
                    match &interface.owner_interface {
                        Some(owner) => Some(format!("{owner}/{}", &interface.name)),
                        None => Some(interface.name.clone()),
                    }
                },
                FunctionMode::Static,
            )?);
        }

        let stub_interface_name = format!("stub-{}", def.source_world_name()?);
        let stub_interface_name = Ident::new(
            &to_rust_ident(&stub_interface_name).to_snake_case(),
            Span::call_site(),
        );

        let constructor = if interface.is_resource() {
            let constructor_stub = FunctionStub {
                name: "new".to_string(),
                params: interface.constructor_params.clone().unwrap_or_default(),
                results: FunctionResultStub::SelfType,
            };
            generate_function_stub_source(
                def,
                &constructor_stub,
                Some(format!(
                    "{}/{}",
                    interface.owner_interface.clone().unwrap_or_default(),
                    &interface.name
                )),
                FunctionMode::Constructor,
            )?
        } else {
            quote! {
                fn new(location: crate::bindings::golem::rpc::types::Uri) -> Self {
                    let location = golem_wasm_rpc::Uri { value: location.value };
                    Self {
                        rpc: WasmRpc::new(&location)
                    }
                }
            }
        };

        interface_impls.push(quote! {
            impl crate::bindings::exports::#root_ns::#root_name::#stub_interface_name::#guest_interface_name for #interface_name {
                #constructor

                #(#fn_impls)*
            }
        });

        if interface.is_resource() {
            let remote_function_name = get_remote_function_name(
                def,
                "drop",
                Some(&format!(
                    "{}/{}",
                    interface.owner_interface.clone().unwrap_or_default(),
                    &interface.name
                )),
            );
            interface_impls.push(quote! {
                impl Drop for #interface_name {
                    fn drop(&mut self) {
                        self.rpc.invoke_and_await(
                            #remote_function_name,
                            &[
                                WitValue::builder().handle(self.uri.clone(), self.id)
                            ]
                        ).expect("Failed to invoke remote drop");
                    }
                }
            });
        }
    }

    let lib = quote! {
        #![allow(warnings)]

        use golem_wasm_rpc::*;

        #[allow(dead_code)]
        mod bindings;

        #(#struct_defs)*

        #(#interface_impls)*
    };

    let syntax_tree = syn::parse2(lib)?;
    let src = prettyplease::unparse(&syntax_tree);

    println!(
        "Generating stub source to {}",
        def.target_rust_path().to_string_lossy()
    );
    fs::create_dir_all(def.target_rust_path().parent().unwrap())?;
    fs::write(def.target_rust_path(), src)?;
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FunctionMode {
    Global,
    Static,
    Method,
    Constructor,
}

fn generate_function_stub_source(
    def: &StubDefinition,
    function: &FunctionStub,
    interface_name: Option<String>,
    mode: FunctionMode,
) -> anyhow::Result<TokenStream> {
    let function_name = Ident::new(&to_rust_ident(&function.name), Span::call_site());
    let mut params = Vec::new();
    let mut input_values = Vec::new();
    let mut output_values = Vec::new();

    if mode != FunctionMode::Static && mode != FunctionMode::Constructor {
        params.push(quote! {&self});
    }

    if mode == FunctionMode::Constructor {
        params.push(quote! { location: crate::bindings::golem::rpc::types::Uri });
    }

    if mode == FunctionMode::Method {
        input_values.push(quote! {
            WitValue::builder().handle(self.uri.clone(), self.id)
        });
    }

    for param in &function.params {
        let param_name = Ident::new(&to_rust_ident(&param.name), Span::call_site());
        let param_typ = type_to_rust_ident(&param.typ, &def.resolve)?;
        params.push(quote! {
            #param_name: #param_typ
        });
        let param_name_access = quote! { #param_name };

        input_values.push(wit_value_builder(
            &param.typ,
            &param_name_access,
            &def.resolve,
            quote! { WitValue::builder() },
            false,
        )?);
    }

    let result_type = match &function.results {
        FunctionResultStub::Single(typ) => {
            let typ = type_to_rust_ident(typ, &def.resolve)?;
            quote! {
                #typ
            }
        }
        FunctionResultStub::Multi(params) => {
            let mut results = Vec::new();
            for param in params {
                let param_name = Ident::new(&to_rust_ident(&param.name), Span::call_site());
                let param_typ = type_to_rust_ident(&param.typ, &def.resolve)?;
                results.push(quote! {
                    #param_name: #param_typ
                });
            }
            if results.is_empty() {
                quote! {
                    ()
                }
            } else {
                quote! {
                    (#(#results),*)
                }
            }
        }
        FunctionResultStub::SelfType => quote! { Self },
    };

    match &function.results {
        FunctionResultStub::Single(typ) => {
            output_values.push(extract_from_wit_value(
                typ,
                &def.resolve,
                quote! { result.tuple_element(0).expect("tuple not found") },
            )?);
        }
        FunctionResultStub::Multi(params) => {
            for (n, param) in params.iter().enumerate() {
                output_values.push(extract_from_wit_value(
                    &param.typ,
                    &def.resolve,
                    quote! { result.tuple_element(#n).expect("tuple not found") },
                )?);
            }
        }
        FunctionResultStub::SelfType if mode == FunctionMode::Constructor => {
            output_values.push(quote! {
                {
                    let (uri, id) = result.tuple_element(0).expect("tuple not found").handle().expect("handle not found");
                    Self {
                        rpc,
                        id,
                        uri
                    }
                }
            });
        }
        FunctionResultStub::SelfType => {
            return Err(anyhow!(
                "SelfType result is only supported for constructors"
            ));
        }
    }

    let remote_function_name =
        get_remote_function_name(def, &function.name, interface_name.as_ref());

    let rpc = match mode {
        FunctionMode::Static => {
            let first_param = function
                .params
                .first()
                .ok_or(anyhow!("static function has no params"))?;
            let first_param_ident =
                Ident::new(&to_rust_ident(&first_param.name), Span::call_site());
            quote! { #first_param_ident.rpc }
        }
        FunctionMode::Constructor => {
            quote! { rpc }
        }
        _ => {
            quote! { self.rpc }
        }
    };

    let init = if mode == FunctionMode::Constructor {
        quote! {
            let location = golem_wasm_rpc::Uri { value: location.value };
            let rpc = WasmRpc::new(&location);
        }
    } else {
        quote! {}
    };

    let blocking = {
        let function_name = if function.results.is_empty() {
            Ident::new(
                &to_rust_ident(&format!("blocking-{}", function.name)),
                Span::call_site(),
            )
        } else {
            function_name.clone()
        };
        quote! {
            fn #function_name(#(#params),*) -> #result_type {
                #init
                let result = #rpc.invoke_and_await(
                    #remote_function_name,
                    &[
                        #(#input_values),*
                    ],
                ).expect(&format!("Failed to invoke-and-await remote {}", #remote_function_name));
                (#(#output_values),*)
            }
        }
    };

    let non_blocking = if function.results.is_empty() {
        quote! {
            fn #function_name(#(#params),*) -> #result_type {
                #init
                let result = #rpc.invoke(
                    #remote_function_name,
                    &[
                        #(#input_values),*
                    ],
                ).expect(&format!("Failed to invoke remote {}", #remote_function_name));
                (#(#output_values),*)
            }
        }
    } else {
        quote! {}
    };

    Ok(quote! {
        #blocking
        #non_blocking
    })
}

fn get_remote_function_name(
    def: &StubDefinition,
    function_name: &str,
    interface_name: Option<&String>,
) -> String {
    match interface_name {
        Some(remote_interface) => format!(
            "{}:{}/{}.{{{}}}",
            def.root_package_name.namespace,
            def.root_package_name.name,
            remote_interface,
            function_name
        ),
        None => function_name.to_string(),
    }
}

fn type_to_rust_ident(typ: &Type, resolve: &Resolve) -> anyhow::Result<TokenStream> {
    match typ {
        Type::Bool => Ok(quote! { bool }),
        Type::U8 => Ok(quote! { u8 }),
        Type::U16 => Ok(quote! { u16 }),
        Type::U32 => Ok(quote! { u32 }),
        Type::U64 => Ok(quote! { u64 }),
        Type::S8 => Ok(quote! { i8 }),
        Type::S16 => Ok(quote! { i16 }),
        Type::S32 => Ok(quote! { i32 }),
        Type::S64 => Ok(quote! { i64 }),
        Type::Float32 => Ok(quote! { f32 }),
        Type::Float64 => Ok(quote! { f64 }),
        Type::Char => Ok(quote! { char }),
        Type::String => Ok(quote! { String }),
        Type::Id(type_id) => {
            let typedef = resolve
                .types
                .get(*type_id)
                .ok_or(anyhow!("type not found"))?;

            match &typedef.kind {
                TypeDefKind::Option(inner) => {
                    let inner = type_to_rust_ident(inner, resolve)?;
                    Ok(quote! { Option<#inner> })
                }
                TypeDefKind::List(inner) => {
                    let inner = type_to_rust_ident(inner, resolve)?;
                    Ok(quote! { Vec<#inner> })
                }
                TypeDefKind::Tuple(tuple) => {
                    let types = tuple
                        .types
                        .iter()
                        .map(|t| type_to_rust_ident(t, resolve))
                        .collect::<anyhow::Result<Vec<_>>>()?;
                    Ok(quote! { (#(#types),*) })
                }
                TypeDefKind::Result(result) => {
                    let ok = match &result.ok {
                        Some(ok) => type_to_rust_ident(ok, resolve)?,
                        None => quote! { () },
                    };
                    let err = match &result.err {
                        Some(err) => type_to_rust_ident(err, resolve)?,
                        None => quote! { () },
                    };
                    Ok(quote! { Result<#ok, #err> })
                }
                TypeDefKind::Handle(handle) => {
                    let (type_id, is_ref) = match handle {
                        Handle::Own(type_id) => (type_id, false),
                        Handle::Borrow(type_id) => (type_id, true),
                    };

                    let ident = resource_type_ident(type_id, resolve)?;
                    if is_ref {
                        Ok(quote! { &#ident })
                    } else {
                        Ok(quote! { wit_bindgen::rt::Resource<#ident> })
                    }
                }
                _ => {
                    let typ = Ident::new(
                        &to_rust_ident(typedef.name.as_ref().ok_or(anyhow!("type has no name"))?)
                            .to_upper_camel_case(),
                        Span::call_site(),
                    );
                    let mut path = Vec::new();
                    path.push(quote! { crate });
                    path.push(quote! { bindings });
                    match &typedef.owner {
                        TypeOwner::World(world_id) => {
                            let world = resolve
                                .worlds
                                .get(*world_id)
                                .ok_or(anyhow!("type's owner world not found"))?;
                            let package_id =
                                world.package.ok_or(anyhow!("world has no package"))?;
                            let package = resolve
                                .packages
                                .get(package_id)
                                .ok_or(anyhow!("package not found"))?;
                            let ns_ident = Ident::new(
                                &to_rust_ident(&package.name.namespace),
                                Span::call_site(),
                            );
                            let name_ident =
                                Ident::new(&to_rust_ident(&package.name.name), Span::call_site());
                            path.push(quote! { #ns_ident });
                            path.push(quote! { #name_ident });
                        }
                        TypeOwner::Interface(interface_id) => {
                            let interface = resolve
                                .interfaces
                                .get(*interface_id)
                                .ok_or(anyhow!("type's owner interface not found"))?;

                            let package_id = interface
                                .package
                                .ok_or(anyhow!("interface has no package"))?;
                            let package = resolve
                                .packages
                                .get(package_id)
                                .ok_or(anyhow!("package not found"))?;
                            let interface_name = interface
                                .name
                                .as_ref()
                                .ok_or(anyhow!("interface has no name"))?;
                            let ns_ident = Ident::new(
                                &to_rust_ident(&package.name.namespace),
                                Span::call_site(),
                            );
                            let name_ident =
                                Ident::new(&to_rust_ident(&package.name.name), Span::call_site());
                            let interface_ident =
                                Ident::new(&to_rust_ident(interface_name), Span::call_site());
                            path.push(quote! { #ns_ident });
                            path.push(quote! { #name_ident });
                            path.push(quote! { #interface_ident });
                        }
                        TypeOwner::None => {}
                    }
                    Ok(quote! { #(#path)::*::#typ })
                }
            }
        }
    }
}

fn resource_type_ident(type_id: &TypeId, resolve: &Resolve) -> anyhow::Result<Ident> {
    let typedef = resolve
        .types
        .get(*type_id)
        .ok_or(anyhow!("type not found"))?;

    let ident = Ident::new(
        &to_rust_ident(
            typedef
                .name
                .as_ref()
                .ok_or(anyhow!("Handle's inner type has no name"))?,
        )
        .to_upper_camel_case(),
        Span::call_site(),
    );
    Ok(ident)
}

fn wit_value_builder(
    typ: &Type,
    name: &TokenStream,
    resolve: &Resolve,
    builder_expr: TokenStream,
    is_reference: bool,
) -> anyhow::Result<TokenStream> {
    match typ {
        Type::Bool => {
            if is_reference {
                Ok(quote! {
                    #builder_expr.bool(*#name)
                })
            } else {
                Ok(quote! {
                    #builder_expr.bool(#name)
                })
            }
        }
        Type::U8 => {
            if is_reference {
                Ok(quote! {
                    #builder_expr.u8(*#name)
                })
            } else {
                Ok(quote! {
                    #builder_expr.u8(#name)
                })
            }
        }
        Type::U16 => {
            if is_reference {
                Ok(quote! {
                    #builder_expr.u16(*#name)
                })
            } else {
                Ok(quote! {
                    #builder_expr.u16(#name)
                })
            }
        }
        Type::U32 => {
            if is_reference {
                Ok(quote! {
                    #builder_expr.u32(*#name)
                })
            } else {
                Ok(quote! {
                    #builder_expr.u32(#name)
                })
            }
        }
        Type::U64 => {
            if is_reference {
                Ok(quote! {
                    #builder_expr.u64(*#name)
                })
            } else {
                Ok(quote! {
                    #builder_expr.u64(#name)
                })
            }
        }
        Type::S8 => {
            if is_reference {
                Ok(quote! {
                    #builder_expr.s8(*#name)
                })
            } else {
                Ok(quote! {
                    #builder_expr.s8(#name)
                })
            }
        }
        Type::S16 => {
            if is_reference {
                Ok(quote! {
                    #builder_expr.s16(*#name)
                })
            } else {
                Ok(quote! {
                    #builder_expr.s16(#name)
                })
            }
        }
        Type::S32 => {
            if is_reference {
                Ok(quote! {
                    #builder_expr.s32(*#name)
                })
            } else {
                Ok(quote! {
                    #builder_expr.s32(#name)
                })
            }
        }
        Type::S64 => {
            if is_reference {
                Ok(quote! {
                    #builder_expr.s64(*#name)
                })
            } else {
                Ok(quote! {
                    #builder_expr.s64(#name)
                })
            }
        }
        Type::Float32 => {
            if is_reference {
                Ok(quote! {
                    #builder_expr.f32(*#name)
                })
            } else {
                Ok(quote! {
                    #builder_expr.f32(#name)
                })
            }
        }
        Type::Float64 => {
            if is_reference {
                Ok(quote! {
                    #builder_expr.f64(*#name)
                })
            } else {
                Ok(quote! {
                    #builder_expr.f64(#name)
                })
            }
        }
        Type::Char => {
            if is_reference {
                Ok(quote! {
                    #builder_expr.char(*#name)
                })
            } else {
                Ok(quote! {
                    #builder_expr.char(#name)
                })
            }
        }
        Type::String => {
            if is_reference {
                Ok(quote! {
                    #builder_expr.string(#name)
                })
            } else {
                Ok(quote! {
                    #builder_expr.string(&#name)
                })
            }
        }
        Type::Id(type_id) => {
            let typedef = resolve
                .types
                .get(*type_id)
                .ok_or(anyhow!("type not found"))?;
            match &typedef.kind {
                TypeDefKind::Record(record) => {
                    wit_record_value_builder(record, name, resolve, builder_expr)
                }
                TypeDefKind::Resource => Err(anyhow!("Resource cannot directly appear in a function signature, just through a Handle")),
                TypeDefKind::Handle(_) => {
                    Ok(quote! {
                        #builder_expr.handle(#name.uri.clone(), #name.id)
                    })
                }
                TypeDefKind::Flags(flags) => {
                    wit_flags_value_builder(flags, typ, name, resolve, builder_expr)
                }
                TypeDefKind::Tuple(tuple) => {
                    wit_tuple_value_builder(tuple, name, resolve, builder_expr)
                }
                TypeDefKind::Variant(variant) => {
                    wit_variant_value_builder(variant, typ, name, resolve, builder_expr)
                }
                TypeDefKind::Enum(enum_def) => {
                    wit_enum_value_builder(enum_def, typ, name, resolve, builder_expr)
                }
                TypeDefKind::Option(inner) => {
                    wit_option_value_builder(inner, name, resolve, builder_expr)
                }
                TypeDefKind::Result(result) => {
                    wit_result_value_builder(result, name, resolve, builder_expr)
                }
                TypeDefKind::List(elem) => {
                    wit_list_value_builder(elem, name, resolve, builder_expr)
                }
                TypeDefKind::Future(_) => Ok(quote!(todo!("future"))),
                TypeDefKind::Stream(_) => Ok(quote!(todo!("stream"))),
                TypeDefKind::Type(typ) => wit_value_builder(typ, name, resolve, builder_expr, is_reference),
                TypeDefKind::Unknown => Ok(quote!(todo!("unknown"))),
            }
        }
    }
}

fn wit_record_value_builder(
    record: &Record,
    name: &TokenStream,
    resolve: &Resolve,
    mut builder_expr: TokenStream,
) -> anyhow::Result<TokenStream> {
    builder_expr = quote! { #builder_expr.record() };

    for field in &record.fields {
        let field_name = Ident::new(&to_rust_ident(&field.name), Span::call_site());
        let field_access = quote! { #name.#field_name };
        builder_expr = wit_value_builder(
            &field.ty,
            &field_access,
            resolve,
            quote! { #builder_expr.item() },
            false,
        )?;
    }

    Ok(quote! { #builder_expr.finish() })
}

fn wit_flags_value_builder(
    flags: &Flags,
    typ: &Type,
    name: &TokenStream,
    resolve: &Resolve,
    builder_expr: TokenStream,
) -> anyhow::Result<TokenStream> {
    let flags_type = type_to_rust_ident(typ, resolve)?;

    let mut flags_vec_values = Vec::new();
    for flag in &flags.flags {
        let flag_id = Ident::new(&flag.name.to_shouty_snake_case(), Span::call_site());
        flags_vec_values.push(quote! {
            (#name & #flags_type::#flag_id) == #flags_type::#flag_id
        })
    }

    Ok(quote! { #builder_expr.flags(vec![#(#flags_vec_values),*]) })
}

fn wit_tuple_value_builder(
    tuple: &Tuple,
    name: &TokenStream,
    resolve: &Resolve,
    mut builder_expr: TokenStream,
) -> anyhow::Result<TokenStream> {
    builder_expr = quote! { #builder_expr.tuple() };

    for (n, typ) in tuple.types.iter().enumerate() {
        let field_name = syn::Index::from(n);
        let field_access = quote! { #name.#field_name };
        builder_expr = wit_value_builder(
            typ,
            &field_access,
            resolve,
            quote! { #builder_expr.item() },
            false,
        )?;
    }

    Ok(quote! { #builder_expr.finish() })
}

fn wit_variant_value_builder(
    variant: &Variant,
    typ: &Type,
    name: &TokenStream,
    resolve: &Resolve,
    builder_expr: TokenStream,
) -> anyhow::Result<TokenStream> {
    let variant_type = type_to_rust_ident(typ, resolve)?;

    let mut case_idx_patterns = Vec::new();
    let mut is_unit_patterns = Vec::new();
    let mut builder_patterns = Vec::new();
    for (n, case) in variant.cases.iter().enumerate() {
        let case_name = Ident::new(
            &to_rust_ident(&case.name).to_upper_camel_case(),
            Span::call_site(),
        );

        let case_idx: u32 = n as u32;

        match &case.ty {
            None => {
                case_idx_patterns.push(quote! {
                    #variant_type::#case_name => #case_idx
                });
                is_unit_patterns.push(quote! {
                    #variant_type::#case_name => true
                });
                builder_patterns.push(quote! {
                    #variant_type::#case_name => {
                        unreachable!()
                    }
                });
            }
            Some(inner_ty) => {
                let inner_builder_expr = wit_value_builder(
                    inner_ty,
                    &quote! { inner },
                    resolve,
                    quote! { case_builder },
                    true,
                )?;

                case_idx_patterns.push(quote! {
                    #variant_type::#case_name(_) => #case_idx
                });
                is_unit_patterns.push(quote! {
                    #variant_type::#case_name(_) => false
                });
                builder_patterns.push(quote! {
                    #variant_type::#case_name(inner) => {
                        #inner_builder_expr
                    }
                });
            }
        }
    }

    Ok(quote! {
        #builder_expr.variant_fn(
            match &#name {
                #(#case_idx_patterns),*,
            },
            match &#name {
                #(#is_unit_patterns),*,
            },
            |case_builder| match &#name {
                #(#builder_patterns),*,
            }
        )
    })
}

fn wit_enum_value_builder(
    enum_def: &Enum,
    typ: &Type,
    name: &TokenStream,
    resolve: &Resolve,
    builder_expr: TokenStream,
) -> anyhow::Result<TokenStream> {
    let enum_type = type_to_rust_ident(typ, resolve)?;

    let mut cases = Vec::new();
    for (n, case) in enum_def.cases.iter().enumerate() {
        let case_name = Ident::new(&case.name.to_upper_camel_case(), Span::call_site());
        let case_idx = n as u32;
        cases.push(quote! {
            #enum_type::#case_name => #case_idx
        });
    }

    Ok(quote! {
        #builder_expr.enum_value(match #name {
            #(#cases),*
        })
    })
}

fn wit_option_value_builder(
    inner: &Type,
    name: &TokenStream,
    resolve: &Resolve,
    builder_expr: TokenStream,
) -> anyhow::Result<TokenStream> {
    let inner_builder_expr = wit_value_builder(
        inner,
        &quote! { #name.as_ref().unwrap() },
        resolve,
        quote! { some_builder },
        true,
    )?;

    Ok(quote! {
        #builder_expr.option_fn(#name.is_some(), |some_builder| {
            #inner_builder_expr
        })
    })
}

fn wit_result_value_builder(
    result: &Result_,
    name: &TokenStream,
    resolve: &Resolve,
    builder_expr: TokenStream,
) -> anyhow::Result<TokenStream> {
    let ok_expr = match &result.ok {
        Some(ok) => wit_value_builder(
            ok,
            &quote! { ok_value },
            resolve,
            quote! { result_builder },
            true,
        )?,
        None => quote! { unreachable!() },
    };
    let err_expr = match &result.err {
        Some(err) => wit_value_builder(
            err,
            &quote! { err_value },
            resolve,
            quote! { result_builder },
            true,
        )?,
        None => quote! { unreachable!() },
    };

    let has_ok = result.ok.is_some();
    let has_err = result.err.is_some();
    Ok(quote! {
        #builder_expr.result_fn(#name.is_ok(), #has_ok, #has_err, |result_builder| {
            match &#name {
                Ok(ok_value) => {
                    #ok_expr
                }
                Err(err_value) => {
                    #err_expr
                }
            }
        })
    })
}

fn wit_list_value_builder(
    inner: &Type,
    name: &TokenStream,
    resolve: &Resolve,
    builder_expr: TokenStream,
) -> anyhow::Result<TokenStream> {
    let inner_builder_expr = wit_value_builder(
        inner,
        &quote! { item },
        resolve,
        quote! { item_builder },
        true,
    )?;

    Ok(quote! {
        #builder_expr.list_fn(&#name, |item, item_builder| {
            #inner_builder_expr
        })
    })
}

fn extract_from_wit_value(
    typ: &Type,
    resolve: &Resolve,
    base_expr: TokenStream,
) -> anyhow::Result<TokenStream> {
    match typ {
        Type::Bool => Ok(quote! {
            #base_expr.bool().expect("bool not found")
        }),
        Type::U8 => Ok(quote! {
            #base_expr.u8().expect("u8 not found")
        }),
        Type::U16 => Ok(quote! {
            #base_expr.u16().expect("u16 not found")
        }),
        Type::U32 => Ok(quote! {
            #base_expr.u32().expect("u32 not found")
        }),
        Type::U64 => Ok(quote! {
            #base_expr.u64().expect("u64 not found")
        }),
        Type::S8 => Ok(quote! {
            #base_expr.s8().expect("i8 not found")
        }),
        Type::S16 => Ok(quote! {
            #base_expr.s16().expect("i16 not found")
        }),
        Type::S32 => Ok(quote! {
            #base_expr.s32().expect("i32 not found")
        }),
        Type::S64 => Ok(quote! {
            #base_expr.s64().expect("i64 not found")
        }),
        Type::Float32 => Ok(quote! {
            #base_expr.f32().expect("f32 not found")
        }),
        Type::Float64 => Ok(quote! {
            #base_expr.f64().expect("f64 not found")
        }),
        Type::Char => Ok(quote! {
            #base_expr.char().expect("char not found")
        }),
        Type::String => Ok(quote! {
            #base_expr.string().expect("string not found").to_string()
        }),
        Type::Id(type_id) => {
            let typedef = resolve
                .types
                .get(*type_id)
                .ok_or(anyhow!("type not found"))?;
            match &typedef.kind {
                TypeDefKind::Record(record) => {
                    extract_from_record_value(record, typ, resolve, base_expr)
                }
                TypeDefKind::Resource => Err(anyhow!("Resource cannot directly appear in a function signature, just through a Handle")),
                TypeDefKind::Handle(handle) => extract_from_handle_value(handle, resolve, base_expr),
                TypeDefKind::Flags(flags) => {
                    extract_from_flags_value(flags, typ, resolve, base_expr)
                }
                TypeDefKind::Tuple(tuple) => extract_from_tuple_value(tuple, resolve, base_expr),
                TypeDefKind::Variant(variant) => {
                    extract_from_variant_value(variant, typ, resolve, base_expr)
                }
                TypeDefKind::Enum(enum_def) => {
                    extract_from_enum_value(enum_def, typ, resolve, base_expr)
                }
                TypeDefKind::Option(inner) => extract_from_option_value(inner, resolve, base_expr),
                TypeDefKind::Result(result) => {
                    extract_from_result_value(result, resolve, base_expr)
                }
                TypeDefKind::List(elem) => extract_from_list_value(elem, resolve, base_expr),
                TypeDefKind::Future(_) => Ok(quote!(todo!("future"))),
                TypeDefKind::Stream(_) => Ok(quote!(todo!("stream"))),
                TypeDefKind::Type(typ) => extract_from_wit_value(typ, resolve, base_expr),
                TypeDefKind::Unknown => Ok(quote!(todo!("unknown"))),
            }
        }
    }
}

fn extract_from_record_value(
    record: &Record,
    record_type: &Type,
    resolve: &Resolve,
    base_expr: TokenStream,
) -> anyhow::Result<TokenStream> {
    let mut field_extractors = Vec::new();
    for (field_idx, field) in record.fields.iter().enumerate() {
        let field_name = Ident::new(&to_rust_ident(&field.name), Span::call_site());
        let field_expr = extract_from_wit_value(
            &field.ty,
            resolve,
            quote! { record.field(#field_idx).expect("record field not found") },
        )?;
        field_extractors.push(quote! {
            #field_name: #field_expr
        });
    }

    let record_type = type_to_rust_ident(record_type, resolve)?;

    Ok(quote! {
        {
            let record = #base_expr;
            #record_type {
                #(#field_extractors),*
            }
        }
    })
}

fn extract_from_flags_value(
    flags: &Flags,
    flags_type: &Type,
    resolve: &Resolve,
    base_expr: TokenStream,
) -> anyhow::Result<TokenStream> {
    let flags_type = type_to_rust_ident(flags_type, resolve)?;
    let mut flag_exprs = Vec::new();
    for (flag_idx, flag) in flags.flags.iter().enumerate() {
        let flag_name = Ident::new(&flag.name.to_shouty_snake_case(), Span::call_site());
        flag_exprs.push(quote! {
            if flag_vec[#flag_idx] {
                flags |= #flags_type::#flag_name;
            }
        });
    }

    Ok(quote! {
        {
            let flag_vec = #base_expr.flags().expect("flags not found");
            let mut flags = #flags_type::empty();
            #(#flag_exprs);*
            flags
        }
    })
}

fn extract_from_tuple_value(
    tuple: &Tuple,
    resolve: &Resolve,
    base_expr: TokenStream,
) -> anyhow::Result<TokenStream> {
    let mut elem_extractors = Vec::new();
    for (field_idx, typ) in tuple.types.iter().enumerate() {
        let elem_expr = extract_from_wit_value(
            typ,
            resolve,
            quote! { tuple.tuple_element(#field_idx).expect("tuple element not found") },
        )?;
        elem_extractors.push(elem_expr);
    }

    Ok(quote! {
        {
            let tuple = #base_expr;
            (#(#elem_extractors),*)
        }
    })
}

fn extract_from_variant_value(
    variant: &Variant,
    variant_type: &Type,
    resolve: &Resolve,
    base_expr: TokenStream,
) -> anyhow::Result<TokenStream> {
    let variant_type = type_to_rust_ident(variant_type, resolve)?;

    let mut case_extractors = Vec::new();
    for (n, case) in variant.cases.iter().enumerate() {
        let case_name = Ident::new(
            &to_rust_ident(&case.name).to_upper_camel_case(),
            Span::call_site(),
        );
        let case_idx = n as u32;

        match &case.ty {
            Some(ty) => {
                let case_expr = extract_from_wit_value(
                    ty,
                    resolve,
                    quote! { inner.expect("variant case not found") },
                )?;
                case_extractors.push(quote! {
                    #case_idx => #variant_type::#case_name(#case_expr)
                });
            }
            None => {
                case_extractors.push(quote! {
                    #case_idx => #variant_type::#case_name
                });
            }
        }
    }

    Ok(quote! {
        {
            let (case_idx, inner) = #base_expr.variant().expect("variant not found");
            match case_idx {
                #(#case_extractors),*,
                _ => unreachable!("invalid variant case index")
            }
        }
    })
}

fn extract_from_enum_value(
    enum_def: &Enum,
    enum_type: &Type,
    resolve: &Resolve,
    base_expr: TokenStream,
) -> anyhow::Result<TokenStream> {
    let enum_type = type_to_rust_ident(enum_type, resolve)?;

    let mut case_extractors = Vec::new();
    for (n, case) in enum_def.cases.iter().enumerate() {
        let case_name = Ident::new(&case.name.to_upper_camel_case(), Span::call_site());
        let case_idx = n as u32;
        case_extractors.push(quote! {
            #case_idx => #enum_type::#case_name
        });
    }

    Ok(quote! {
        {
            let case_idx = #base_expr.enum_value().expect("enum not found");
            match case_idx {
                #(#case_extractors),*,
                _ => unreachable!("invalid enum case index")
            }
        }
    })
}

fn extract_from_option_value(
    inner: &Type,
    resolve: &Resolve,
    base_expr: TokenStream,
) -> anyhow::Result<TokenStream> {
    let inner_expr = extract_from_wit_value(inner, resolve, quote! { inner })?;

    Ok(quote! {
        #base_expr.option().expect("option not found").map(|inner| #inner_expr)
    })
}

fn extract_from_result_value(
    result: &Result_,
    resolve: &Resolve,
    base_expr: TokenStream,
) -> anyhow::Result<TokenStream> {
    let ok_expr = match &result.ok {
        Some(ok) => extract_from_wit_value(
            ok,
            resolve,
            quote! { ok_value.expect("result ok value not found") },
        )?,
        None => quote! { () },
    };
    let err_expr = match &result.err {
        Some(err) => extract_from_wit_value(
            err,
            resolve,
            quote! { err_value.expect("result err value not found") },
        )?,
        None => quote! { () },
    };

    Ok(quote! {
        {
            let result = #base_expr.result().expect("result not found");
            match result {
                Ok(ok_value) => Ok(#ok_expr),
                Err(err_value) => Err(#err_expr)
            }
        }
    })
}

fn extract_from_list_value(
    inner: &Type,
    resolve: &Resolve,
    base_expr: TokenStream,
) -> anyhow::Result<TokenStream> {
    let inner_expr = extract_from_wit_value(inner, resolve, quote! { item })?;

    Ok(quote! {
        #base_expr.list_elements(|item| #inner_expr).expect("list not found")
    })
}

fn extract_from_handle_value(
    handle: &Handle,
    resolve: &Resolve,
    base_expr: TokenStream,
) -> anyhow::Result<TokenStream> {
    match handle {
        Handle::Own(type_id) => {
            let ident = resource_type_ident(type_id, resolve)?;
            Ok(quote! {
                {
                    let (uri, id) = #base_expr.handle().expect("handle not found");
                    wit_bindgen::rt::Resource::new(#ident::from_remote_handle(uri, id))
                }
            })
        }
        Handle::Borrow(type_id) => {
            let ident = resource_type_ident(type_id, resolve)?;
            Ok(quote! {
                {
                    let (uri, id) = #base_expr.handle().expect("handle not found");
                    #ident::from_remote_handle(uri, id)
                }
            })
        }
    }
}
