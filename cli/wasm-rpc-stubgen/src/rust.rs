// Copyright 2024-2025 Golem Cloud
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

use crate::fs;
use crate::fs::PathExtra;
use crate::log::{log_action, LogColorize};
use crate::stub::{FunctionResultStub, FunctionStub, InterfaceStub, StubDefinition};
use anyhow::anyhow;
use heck::{ToShoutySnakeCase, ToSnakeCase, ToUpperCamelCase};
use proc_macro2::{Ident, Span, TokenStream};
use quote::quote;
use wit_bindgen_rust::to_rust_ident;
use wit_parser::{
    Enum, Flags, Handle, Record, Result_, Tuple, Type, TypeDef, TypeDefKind, TypeOwner, Variant,
};

pub fn generate_stub_source(def: &StubDefinition) -> anyhow::Result<()> {
    let root_ns = Ident::new(
        &def.source_package_name.namespace.to_snake_case(),
        Span::call_site(),
    );

    let root_name = Ident::new(
        &format!("{}_stub", def.source_package_name.name.to_snake_case()),
        Span::call_site(),
    );
    let stub_interface_name = format!("stub-{}", def.source_world_name());
    let stub_interface_name = Ident::new(
        &to_rust_ident(&stub_interface_name).to_snake_case(),
        Span::call_site(),
    );

    let mut struct_defs = Vec::new();
    let mut exports = Vec::new();
    let mut resource_type_aliases = Vec::new();

    for interface in def.stub_imported_interfaces() {
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

        resource_type_aliases.push(quote! {
            type #interface_name = crate::#interface_name;
        });

        for function in &interface.functions {
            if !function.results.is_empty() {
                let result_wrapper = result_wrapper_ident(function, interface);
                struct_defs.push(quote! {
                    pub struct #result_wrapper {
                        pub future_invoke_result: FutureInvokeResult
                    }
                });

                resource_type_aliases.push(quote! {
                    type #result_wrapper = crate::#result_wrapper;
                });
            }
        }
        for function in &interface.static_functions {
            if !function.results.is_empty() {
                let result_wrapper = result_wrapper_ident(function, interface);
                struct_defs.push(quote! {
                    pub struct #result_wrapper {
                        pub future_invoke_result: FutureInvokeResult
                    }
                });

                resource_type_aliases.push(quote! {
                    type #result_wrapper = crate::#result_wrapper;
                });
            }
        }
    }

    let mut interface_impls = Vec::new();
    for interface in def.stub_imported_interfaces() {
        let interface_ident = to_rust_ident(&interface.name).to_upper_camel_case();
        let interface_name = Ident::new(&interface_ident, Span::call_site());
        let guest_interface_name =
            Ident::new(&format!("Guest{}", interface_ident), Span::call_site());

        let mut fn_impls = Vec::new();
        for function in &interface.functions {
            let mode = if interface.is_resource() {
                FunctionMode::Method
            } else {
                FunctionMode::Global
            };
            fn_impls.push(generate_function_stub_source(
                def, function, interface, mode,
            )?);

            if !function.results.is_empty() {
                let result_wrapper = result_wrapper_ident(function, interface);
                let result_wrapper_interface = result_wrapper_interface_ident(function, interface);

                let subscribe = quote! {
                    fn subscribe(&self) -> bindings::wasi::io::poll::Pollable {
                        let pollable = self.future_invoke_result.subscribe();
                        let pollable = unsafe {
                            bindings::wasi::io::poll::Pollable::from_handle(
                                pollable.take_handle()
                            )
                        };
                        pollable
                    }
                };

                let get = generate_result_wrapper_get_source(def, interface, function, mode)?;

                let result_wrapper_impl = quote! {
                    impl crate::bindings::exports::#root_ns::#root_name::#stub_interface_name::#result_wrapper_interface for #result_wrapper {
                        #subscribe
                        #get
                    }
                };
                interface_impls.push(result_wrapper_impl);
            }
        }

        for function in &interface.static_functions {
            fn_impls.push(generate_function_stub_source(
                def,
                function,
                interface,
                FunctionMode::Static,
            )?);

            if !function.results.is_empty() {
                let result_wrapper = result_wrapper_ident(function, interface);
                let result_wrapper_interface = result_wrapper_interface_ident(function, interface);

                let subscribe = quote! {
                    fn subscribe(&self) -> bindings::wasi::io::poll::Pollable {
                        let pollable = self.future_invoke_result.subscribe();
                        let pollable = unsafe {
                            bindings::wasi::io::poll::Pollable::from_handle(
                                pollable.take_handle()
                            )
                        };
                        pollable
                    }
                };

                let get = generate_result_wrapper_get_source(
                    def,
                    interface,
                    function,
                    FunctionMode::Static,
                )?;

                let result_wrapper_impl = quote! {
                    impl crate::bindings::exports::#root_ns::#root_name::#stub_interface_name::#result_wrapper_interface for #result_wrapper {
                        #subscribe
                        #get
                    }
                };
                interface_impls.push(result_wrapper_impl);
            }
        }

        let constructor = if interface.is_resource() {
            let constructor_stub = FunctionStub {
                name: "new".to_string(),
                params: interface.constructor_params.clone().unwrap_or_default(),
                results: FunctionResultStub::SelfType,
            };
            generate_function_stub_source(
                def,
                &constructor_stub,
                interface,
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

        let remote_function_name = get_remote_function_name(
            interface.interface_name(),
            interface.resource_name(),
            "drop",
        );
        if interface.is_resource() {
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

    struct_defs.push(quote! {
        struct Component;

        impl crate::bindings::exports::#root_ns::#root_name::#stub_interface_name::Guest for Component {
            #(#resource_type_aliases)*
        }
    });
    exports.push(quote! {
       bindings::export!(Component with_types_in bindings);
    });

    let lib = quote! {
        #![allow(warnings)]

        use golem_wasm_rpc::*;

        #[allow(dead_code)]
        mod bindings;

        #(#struct_defs)*

        #(#interface_impls)*

        #(#exports)*
    };

    let syntax_tree = syn::parse2(lib)?;
    let src = prettyplease::unparse(&syntax_tree);

    let target_rust_path = PathExtra::new(def.target_rust_path());

    log_action(
        "Generating",
        format!("stub source to {}", target_rust_path.log_color_highlight()),
    );
    fs::create_dir_all(target_rust_path.parent()?)?;
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

fn result_wrapper_ident(function: &FunctionStub, owner: &InterfaceStub) -> Ident {
    Ident::new(
        &to_rust_ident(&function.async_result_type(owner)).to_upper_camel_case(),
        Span::call_site(),
    )
}

fn result_wrapper_interface_ident(function: &FunctionStub, owner: &InterfaceStub) -> Ident {
    Ident::new(
        &to_rust_ident(&format!("guest-{}", function.async_result_type(owner)))
            .to_upper_camel_case(),
        Span::call_site(),
    )
}

fn generate_result_wrapper_get_source(
    def: &StubDefinition,
    interface: &InterfaceStub,
    function: &FunctionStub,
    mode: FunctionMode,
) -> anyhow::Result<TokenStream> {
    let result_type = get_result_type_source(def, function)?;
    let output_values = get_output_values_source(def, function, mode)?;

    let remote_function_name = get_remote_function_name(
        interface.interface_name(),
        interface.resource_name(),
        &function.name,
    );

    Ok(quote! {
        fn get(&self) -> Option<#result_type> {
            self.future_invoke_result.get().map(|result| {
                let result = result.expect(&format!("Failed to invoke remote {}", #remote_function_name));
                (#(#output_values),*)
            })
        }
    })
}

fn generate_function_stub_source(
    def: &StubDefinition,
    function: &FunctionStub,
    owner: &InterfaceStub,
    mode: FunctionMode,
) -> anyhow::Result<TokenStream> {
    let function_name = Ident::new(&to_rust_ident(&function.name), Span::call_site());
    let mut params = Vec::new();
    let mut input_values = Vec::new();

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
        let param_typ = type_to_rust_ident(&param.typ, def)?;
        params.push(quote! {
            #param_name: #param_typ
        });
        let param_name_access = quote! { #param_name };

        input_values.push(wit_value_builder(
            &param.typ,
            &param_name_access,
            def,
            quote! { WitValue::builder() },
            false,
        )?);
    }

    let result_type = get_result_type_source(def, function)?;
    let output_values = get_output_values_source(def, function, mode)?;

    let remote_function_name = get_remote_function_name(
        owner.interface_name(),
        owner.resource_name(),
        &function.name,
    );

    let rpc = match mode {
        FunctionMode::Static => {
            let first_param = function
                .params
                .first()
                .ok_or(anyhow!("static function has no params"))?;
            let first_param_ident =
                Ident::new(&to_rust_ident(&first_param.name), Span::call_site());

            let type_id = match &first_param.typ {
                Type::Id(type_id) => {
                    let type_def = def.get_type_def(*type_id)?;

                    match &type_def.kind {
                        TypeDefKind::Handle(Handle::Borrow(type_id)) => Ok(*type_id),
                        TypeDefKind::Handle(Handle::Own(type_id)) => Ok(*type_id),
                        _ => Err(anyhow!("first parameter of static method is not a handle")),
                    }
                }
                _ => Err(anyhow!("first parameter of static method is not a handle")),
            }?;
            let first_param_type = resource_type_ident(def.get_type_def(type_id)?)?;

            quote! { #first_param_ident.get::<#first_param_type>().rpc }
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
        let blocking_function_name = if mode == FunctionMode::Constructor {
            function.name.clone()
        } else {
            format!("blocking-{}", function.name)
        };
        let function_name = Ident::new(&to_rust_ident(&blocking_function_name), Span::call_site());
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

    let non_blocking = if mode != FunctionMode::Constructor {
        if function.results.is_empty() {
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
            let root_ns = Ident::new(
                &def.source_package_name.namespace.to_snake_case(),
                Span::call_site(),
            );
            let root_name = Ident::new(
                &format!("{}_stub", def.source_package_name.name.to_snake_case()),
                Span::call_site(),
            );
            let stub_interface_name = format!("stub-{}", def.source_world_name());
            let stub_interface_name = Ident::new(
                &to_rust_ident(&stub_interface_name).to_snake_case(),
                Span::call_site(),
            );
            let result_wrapper = result_wrapper_ident(function, owner);
            quote! {
                fn #function_name(#(#params),*) -> crate::bindings::exports::#root_ns::#root_name::#stub_interface_name::#result_wrapper {
                    #init
                    let result = #rpc.async_invoke_and_await(
                        #remote_function_name,
                        &[
                            #(#input_values),*
                        ],
                    );
                    crate::bindings::exports::#root_ns::#root_name::#stub_interface_name::#result_wrapper::new(#result_wrapper { future_invoke_result: result})
                }
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

fn get_output_values_source(
    def: &StubDefinition,
    function: &FunctionStub,
    mode: FunctionMode,
) -> anyhow::Result<Vec<TokenStream>> {
    let mut output_values = Vec::new();
    match &function.results {
        FunctionResultStub::Anon(typ) => {
            output_values.push(extract_from_wit_value(
                typ,
                def,
                quote! { result.tuple_element(0).expect("tuple not found") },
            )?);
        }
        FunctionResultStub::Named(params) => {
            for (n, param) in params.iter().enumerate() {
                output_values.push(extract_from_wit_value(
                    &param.typ,
                    def,
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
    Ok(output_values)
}

fn get_result_type_source(
    def: &StubDefinition,
    function: &FunctionStub,
) -> anyhow::Result<TokenStream> {
    let result_type = match &function.results {
        FunctionResultStub::Anon(typ) => {
            let typ = type_to_rust_ident(typ, def)?;
            quote! {
                #typ
            }
        }
        FunctionResultStub::Named(params) => {
            let mut results = Vec::new();
            for param in params {
                let param_name = Ident::new(&to_rust_ident(&param.name), Span::call_site());
                let param_typ = type_to_rust_ident(&param.typ, def)?;
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
    Ok(result_type)
}

fn get_remote_function_name(
    interface_name: Option<&str>,
    resource_name: Option<&str>,
    function_name: &str,
) -> String {
    match (interface_name, resource_name) {
        (Some(remote_interface), None) => {
            format!("{}.{{{}}}", remote_interface, function_name)
        }
        (Some(remote_interface), Some(resource)) => {
            format!("{}.{{{}.{}}}", remote_interface, resource, function_name)
        }
        (None, Some(resource)) => {
            format!("{}.{}", resource, function_name)
        }
        (None, None) => function_name.to_string(),
    }
}

fn type_to_rust_ident(typ: &Type, def: &StubDefinition) -> anyhow::Result<TokenStream> {
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
        Type::F32 => Ok(quote! { f32 }),
        Type::F64 => Ok(quote! { f64 }),
        Type::Char => Ok(quote! { char }),
        Type::String => Ok(quote! { String }),
        Type::Id(type_id) => {
            let type_def = def.get_type_def(*type_id)?;

            match &type_def.kind {
                TypeDefKind::Option(inner) => {
                    let inner = type_to_rust_ident(inner, def)?;
                    Ok(quote! { Option<#inner> })
                }
                TypeDefKind::List(inner) => {
                    let inner = type_to_rust_ident(inner, def)?;
                    Ok(quote! { Vec<#inner> })
                }
                TypeDefKind::Tuple(tuple) => {
                    let types = tuple
                        .types
                        .iter()
                        .map(|t| type_to_rust_ident(t, def))
                        .collect::<anyhow::Result<Vec<_>>>()?;
                    Ok(quote! { (#(#types),*) })
                }
                TypeDefKind::Result(result) => {
                    let ok = match &result.ok {
                        Some(ok) => type_to_rust_ident(ok, def)?,
                        None => quote! { () },
                    };
                    let err = match &result.err {
                        Some(err) => type_to_rust_ident(err, def)?,
                        None => quote! { () },
                    };
                    Ok(quote! { Result<#ok, #err> })
                }
                TypeDefKind::Handle(handle) => {
                    let (type_id, is_ref) = match handle {
                        Handle::Own(type_id) => (type_id, false),
                        Handle::Borrow(type_id) => (type_id, true),
                    };

                    let root_ns = Ident::new(
                        &def.source_package_name.namespace.to_snake_case(),
                        Span::call_site(),
                    );
                    let root_name = Ident::new(
                        &format!("{}_stub", def.source_package_name.name.to_snake_case()),
                        Span::call_site(),
                    );
                    let stub_interface_name = format!("stub-{}", def.source_world_name());
                    let stub_interface_name = Ident::new(
                        &to_rust_ident(&stub_interface_name).to_snake_case(),
                        Span::call_site(),
                    );

                    let ident = resource_type_ident(def.get_type_def(*type_id)?)?;
                    if is_ref {
                        let borrow_ident = Ident::new(
                            &format!(
                                "{}Borrow",
                                to_rust_ident(&ident.to_string()).to_upper_camel_case()
                            ),
                            Span::call_site(),
                        );
                        Ok(
                            quote! { crate::bindings::exports::#root_ns::#root_name::#stub_interface_name::#borrow_ident<'_> },
                        )
                    } else {
                        Ok(
                            quote! { crate::bindings::exports::#root_ns::#root_name::#stub_interface_name::#ident },
                        )
                    }
                }
                _ => {
                    let typ = Ident::new(
                        &to_rust_ident(type_def.name.as_ref().ok_or(anyhow!("type has no name"))?)
                            .to_upper_camel_case(),
                        Span::call_site(),
                    );
                    let mut path = Vec::new();
                    path.push(quote! { crate });
                    path.push(quote! { bindings });
                    match &type_def.owner {
                        TypeOwner::World(world_id) => {
                            let world = def.get_world(*world_id)?;
                            let package_id =
                                world.package.ok_or(anyhow!("world has no package"))?;
                            let package = def.get_package(package_id)?;
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
                            let interface = def.get_interface(*interface_id)?;
                            let package_id = interface
                                .package
                                .ok_or(anyhow!("interface has no package"))?;
                            let package = def.get_package(package_id)?;
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

fn resource_type_ident(type_def: &TypeDef) -> anyhow::Result<Ident> {
    let ident = Ident::new(
        &to_rust_ident(
            type_def
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
    def: &StubDefinition,
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
        Type::F32 => {
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
        Type::F64 => {
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
            let type_def = def.get_type_def(*type_id)?;
            match &type_def.kind {
                TypeDefKind::Record(record) => {
                    wit_record_value_builder(record, name, def, builder_expr)
                }
                TypeDefKind::Resource => Err(anyhow!("Resource cannot directly appear in a function signature, just through a Handle")),
                TypeDefKind::Handle(handle) => {
                    let ident = match handle {
                        Handle::Own(type_id) =>
                            resource_type_ident(def.get_type_def(*type_id)?)?,
                            Handle::Borrow(type_id) =>
                                resource_type_ident(def.get_type_def(*type_id)?)?,
                    };
                    Ok(quote! {
                        #builder_expr.handle(#name.get::<#ident>().uri.clone(), #name.get::<#ident>().id)
                    })
                }
                TypeDefKind::Flags(flags) => {
                    wit_flags_value_builder(flags, typ, name, def, builder_expr)
                }
                TypeDefKind::Tuple(tuple) => {
                    wit_tuple_value_builder(tuple, name, def, builder_expr)
                }
                TypeDefKind::Variant(variant) => {
                    wit_variant_value_builder(variant, typ, name, def, builder_expr)
                }
                TypeDefKind::Enum(enum_def) => {
                    wit_enum_value_builder(enum_def, typ, name, def, builder_expr)
                }
                TypeDefKind::Option(inner) => {
                    wit_option_value_builder(inner, name, def, builder_expr)
                }
                TypeDefKind::Result(result) => {
                    wit_result_value_builder(result, name, def, builder_expr)
                }
                TypeDefKind::List(elem) => {
                    wit_list_value_builder(elem, name, def, builder_expr)
                }
                TypeDefKind::Future(_) => Ok(quote!(todo!("future"))),
                TypeDefKind::Stream(_) => Ok(quote!(todo!("stream"))),
                TypeDefKind::Type(typ) => wit_value_builder(typ, name, def, builder_expr, is_reference),
                TypeDefKind::Unknown => Ok(quote!(todo!("unknown"))),
            }
        }
    }
}

fn wit_record_value_builder(
    record: &Record,
    name: &TokenStream,
    def: &StubDefinition,
    mut builder_expr: TokenStream,
) -> anyhow::Result<TokenStream> {
    builder_expr = quote! { #builder_expr.record() };

    for field in &record.fields {
        let field_name = Ident::new(&to_rust_ident(&field.name), Span::call_site());
        let field_access = quote! { #name.#field_name };
        builder_expr = wit_value_builder(
            &field.ty,
            &field_access,
            def,
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
    def: &StubDefinition,
    builder_expr: TokenStream,
) -> anyhow::Result<TokenStream> {
    let flags_type = type_to_rust_ident(typ, def)?;

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
    def: &StubDefinition,
    mut builder_expr: TokenStream,
) -> anyhow::Result<TokenStream> {
    builder_expr = quote! { #builder_expr.tuple() };

    for (n, typ) in tuple.types.iter().enumerate() {
        let field_name = syn::Index::from(n);
        let field_access = quote! { #name.#field_name };
        builder_expr = wit_value_builder(
            typ,
            &field_access,
            def,
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
    def: &StubDefinition,
    builder_expr: TokenStream,
) -> anyhow::Result<TokenStream> {
    let variant_type = type_to_rust_ident(typ, def)?;

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
                    def,
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
    def: &StubDefinition,
    builder_expr: TokenStream,
) -> anyhow::Result<TokenStream> {
    let enum_type = type_to_rust_ident(typ, def)?;

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
    def: &StubDefinition,
    builder_expr: TokenStream,
) -> anyhow::Result<TokenStream> {
    let inner_builder_expr = wit_value_builder(
        inner,
        &quote! { #name.as_ref().unwrap() },
        def,
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
    def: &StubDefinition,
    builder_expr: TokenStream,
) -> anyhow::Result<TokenStream> {
    let ok_expr = match &result.ok {
        Some(ok) => wit_value_builder(
            ok,
            &quote! { ok_value },
            def,
            quote! { result_builder },
            true,
        )?,
        None => quote! { unreachable!() },
    };
    let err_expr = match &result.err {
        Some(err) => wit_value_builder(
            err,
            &quote! { err_value },
            def,
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
    def: &StubDefinition,
    builder_expr: TokenStream,
) -> anyhow::Result<TokenStream> {
    let inner_builder_expr =
        wit_value_builder(inner, &quote! { item }, def, quote! { item_builder }, true)?;

    Ok(quote! {
        #builder_expr.list_fn(&#name, |item, item_builder| {
            #inner_builder_expr
        })
    })
}

fn extract_from_wit_value(
    typ: &Type,
    def: &StubDefinition,
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
        Type::F32 => Ok(quote! {
            #base_expr.f32().expect("f32 not found")
        }),
        Type::F64 => Ok(quote! {
            #base_expr.f64().expect("f64 not found")
        }),
        Type::Char => Ok(quote! {
            #base_expr.char().expect("char not found")
        }),
        Type::String => Ok(quote! {
            #base_expr.string().expect("string not found").to_string()
        }),
        Type::Id(type_id) => {
            let type_def = def.get_type_def(*type_id)?;
            match &type_def.kind {
                TypeDefKind::Record(record) => {
                    extract_from_record_value(record, typ, def, base_expr)
                }
                TypeDefKind::Resource => Err(anyhow!("Resource cannot directly appear in a function signature, just through a Handle")),
                TypeDefKind::Handle(handle) => extract_from_handle_value(handle, def, base_expr),
                TypeDefKind::Flags(flags) => {
                    extract_from_flags_value(flags, typ, def, base_expr)
                }
                TypeDefKind::Tuple(tuple) => extract_from_tuple_value(tuple, def, base_expr),
                TypeDefKind::Variant(variant) => {
                    extract_from_variant_value(variant, typ, def, base_expr)
                }
                TypeDefKind::Enum(enum_def) => {
                    extract_from_enum_value(enum_def, typ, def, base_expr)
                }
                TypeDefKind::Option(inner) => extract_from_option_value(inner, def, base_expr),
                TypeDefKind::Result(result) => {
                    extract_from_result_value(result, def, base_expr)
                }
                TypeDefKind::List(elem) => extract_from_list_value(elem, def, base_expr),
                TypeDefKind::Future(_) => Ok(quote!(panic!("Future is not supported yet"))),
                TypeDefKind::Stream(_) => Ok(quote!(panic!("Stream is not supported yet"))),
                TypeDefKind::Type(typ) => extract_from_wit_value(typ, def, base_expr),
                TypeDefKind::Unknown => Ok(quote!(panic!("Unexpected unknown type!"))),
            }
        }
    }
}

fn extract_from_record_value(
    record: &Record,
    record_type: &Type,
    def: &StubDefinition,
    base_expr: TokenStream,
) -> anyhow::Result<TokenStream> {
    let mut field_extractors = Vec::new();
    for (field_idx, field) in record.fields.iter().enumerate() {
        let field_name = Ident::new(&to_rust_ident(&field.name), Span::call_site());
        let field_expr = extract_from_wit_value(
            &field.ty,
            def,
            quote! { record.field(#field_idx).expect("record field not found") },
        )?;
        field_extractors.push(quote! {
            #field_name: #field_expr
        });
    }

    let record_type = type_to_rust_ident(record_type, def)?;

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
    def: &StubDefinition,
    base_expr: TokenStream,
) -> anyhow::Result<TokenStream> {
    let flags_type = type_to_rust_ident(flags_type, def)?;
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
    def: &StubDefinition,
    base_expr: TokenStream,
) -> anyhow::Result<TokenStream> {
    let mut elem_extractors = Vec::new();
    for (field_idx, typ) in tuple.types.iter().enumerate() {
        let elem_expr = extract_from_wit_value(
            typ,
            def,
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
    def: &StubDefinition,
    base_expr: TokenStream,
) -> anyhow::Result<TokenStream> {
    let variant_type = type_to_rust_ident(variant_type, def)?;

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
                    def,
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
    def: &StubDefinition,
    base_expr: TokenStream,
) -> anyhow::Result<TokenStream> {
    let enum_type = type_to_rust_ident(enum_type, def)?;

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
    def: &StubDefinition,
    base_expr: TokenStream,
) -> anyhow::Result<TokenStream> {
    let inner_expr = extract_from_wit_value(inner, def, quote! { inner })?;

    Ok(quote! {
        #base_expr.option().expect("option not found").map(|inner| #inner_expr)
    })
}

fn extract_from_result_value(
    result: &Result_,
    def: &StubDefinition,
    base_expr: TokenStream,
) -> anyhow::Result<TokenStream> {
    let ok_expr = match &result.ok {
        Some(ok) => extract_from_wit_value(
            ok,
            def,
            quote! { ok_value.expect("result ok value not found") },
        )?,
        None => quote! { () },
    };
    let err_expr = match &result.err {
        Some(err) => extract_from_wit_value(
            err,
            def,
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
    def: &StubDefinition,
    base_expr: TokenStream,
) -> anyhow::Result<TokenStream> {
    let inner_expr = extract_from_wit_value(inner, def, quote! { item })?;

    Ok(quote! {
        #base_expr.list_elements(|item| #inner_expr).expect("list not found")
    })
}

fn extract_from_handle_value(
    handle: &Handle,
    def: &StubDefinition,
    base_expr: TokenStream,
) -> anyhow::Result<TokenStream> {
    let root_ns = Ident::new(
        &def.source_package_name.namespace.to_snake_case(),
        Span::call_site(),
    );
    let root_name = Ident::new(
        &format!("{}_stub", def.source_package_name.name.to_snake_case()),
        Span::call_site(),
    );
    let stub_interface_name = format!("stub-{}", def.source_world_name());
    let stub_interface_name = Ident::new(
        &to_rust_ident(&stub_interface_name).to_snake_case(),
        Span::call_site(),
    );

    match handle {
        Handle::Own(type_id) => {
            let ident = resource_type_ident(def.get_type_def(*type_id)?)?;
            Ok(quote! {
                {
                    let (uri, id) = #base_expr.handle().expect("handle not found");
                    crate::bindings::exports::#root_ns::#root_name::#stub_interface_name::#ident::new(#ident::from_remote_handle(uri, id))
                }
            })
        }
        Handle::Borrow(type_id) => {
            let ident = resource_type_ident(def.get_type_def(*type_id)?)?;
            let borrow_ident = Ident::new(
                &format!(
                    "{}Borrow",
                    to_rust_ident(&ident.to_string()).to_upper_camel_case()
                ),
                Span::call_site(),
            );
            Ok(quote! {
                {
                    let (uri, id) = #base_expr.handle().expect("handle not found");
                    crate::bindings::exports::#root_ns::#root_name::#stub_interface_name::#borrow_ident::new(#ident::from_remote_handle(uri, id))
                }
            })
        }
    }
}
