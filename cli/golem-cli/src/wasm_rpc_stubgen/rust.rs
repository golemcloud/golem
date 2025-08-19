// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use crate::fs;
use crate::fs::PathExtra;
use crate::log::{log_action, LogColorize};
use crate::wasm_rpc_stubgen::naming;
use crate::wasm_rpc_stubgen::stub::{FunctionResultStub, FunctionStub, StubDefinition};
use crate::wasm_rpc_stubgen::{GOLEM_RPC_WIT_VERSION, WASI_WIT_VERSION};
use anyhow::anyhow;
use heck::{ToShoutySnakeCase, ToUpperCamelCase};
use proc_macro2::{Ident, Span, TokenStream};
use quote::quote;
use semver::Version;
use std::collections::{HashMap, HashSet};
use wit_bindgen_rust::to_rust_ident;
use wit_parser::{
    Enum, Flags, Handle, PackageName, Record, Result_, Tuple, Type, TypeDef, TypeDefKind,
    TypeOwner, Variant,
};

use super::stub::StubbedEntity;

pub fn generate_stub_source(def: &StubDefinition) -> anyhow::Result<()> {
    let root_ns = def.rust_root_namespace();
    let root_name = def.rust_client_root_name();
    let stub_interface_name = def.rust_client_interface_name();

    let mut struct_defs = Vec::new();
    let mut exports = Vec::new();
    let mut resource_type_aliases = Vec::new();

    for entity in def.stubbed_entities() {
        let interface_ident = to_rust_ident(entity.name()).to_upper_camel_case();
        let interface_name = Ident::new(&interface_ident, Span::call_site());

        let additional_fields = if entity.is_resource() {
            vec![quote! {
                id: u64,
                uri: golem_rust::wasm_rpc::Uri
            }]
        } else {
            vec![]
        };
        let struct_fns: Vec<TokenStream> = if entity.is_resource() {
            vec![quote! {
                pub fn from_remote_handle(uri: golem_rust::wasm_rpc::Uri, id: u64) -> Self {
                    let worker_id: golem_rust::wasm_rpc::WorkerId = uri.clone().try_into().expect(
                        &format!("Invalid worker uri in remote resource handle: {}", uri.value)
                    );
                    Self {
                        rpc: WasmRpc::new(&worker_id),
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

        for function in entity.functions() {
            if !function.results.is_empty() {
                let result_wrapper = naming::rust::result_wrapper_ident(function, entity);
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
        for function in entity.static_functions() {
            if !function.results.is_empty() {
                let result_wrapper = naming::rust::result_wrapper_ident(function, entity);
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
    for entity in def.stubbed_entities() {
        let interface_ident = to_rust_ident(entity.name()).to_upper_camel_case();
        let interface_name = Ident::new(&interface_ident, Span::call_site());
        let guest_interface_name =
            Ident::new(&format!("Guest{interface_ident}"), Span::call_site());

        let mut fn_impls = Vec::new();
        for function in entity.functions() {
            let mode = if entity.is_resource() {
                FunctionMode::Method
            } else {
                FunctionMode::Global
            };
            fn_impls.push(generate_function_stub_source(
                def,
                function,
                entity,
                &interface_name,
                mode,
            )?);

            if !function.results.is_empty() {
                let result_wrapper = naming::rust::result_wrapper_ident(function, entity);
                let result_wrapper_interface =
                    naming::rust::result_wrapper_interface_ident(function, entity);

                let subscribe = quote! {
                    fn subscribe(&self) -> golem_rust::wasm_rpc::Pollable {
                        let pollable = self.future_invoke_result.subscribe();
                        let pollable = unsafe {
                            golem_rust::wasm_rpc::Pollable::from_handle(
                                pollable.take_handle()
                            )
                        };
                        pollable
                    }
                };

                let get = generate_result_wrapper_get_source(
                    def,
                    entity,
                    function,
                    &interface_name,
                    mode,
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

        for function in entity.static_functions() {
            fn_impls.push(generate_function_stub_source(
                def,
                function,
                entity,
                &interface_name,
                FunctionMode::Static,
            )?);

            if !function.results.is_empty() {
                let result_wrapper = naming::rust::result_wrapper_ident(function, entity);
                let result_wrapper_interface =
                    naming::rust::result_wrapper_interface_ident(function, entity);

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
                    entity,
                    function,
                    &interface_name,
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

        let constructor = if entity.is_resource() {
            let constructor_stub = FunctionStub {
                name: "new".to_string(),
                params: entity.constructor_params(),
                results: FunctionResultStub::SelfType,
            };
            let default_constructor = generate_function_stub_source(
                def,
                &constructor_stub,
                entity,
                &interface_name,
                FunctionMode::Constructor,
            )?;

            let mut custom_constructor_stub = constructor_stub.clone();
            custom_constructor_stub.name = "custom".to_string();
            let custom_constructor = generate_function_stub_source(
                def,
                &custom_constructor_stub,
                entity,
                &interface_name,
                FunctionMode::CustomConstructor,
            )?;
            quote! {
                #default_constructor
                #custom_constructor
            }
        } else {
            let component_name = def.config.component_name.as_str();

            if def.config.is_ephemeral {
                quote! {
                    fn new() -> Self {
                        let component_name = #component_name;
                        let component_id = golem_rust::bindings::golem::api::host::resolve_component_id(component_name).expect(
                            &format!("Failed to resolve component id: {}", component_name)
                        );
                        Self {
                            rpc: WasmRpc::ephemeral(component_id)
                        }
                    }

                    fn custom(component_id: golem_rust::wasm_rpc:: ComponentId) -> crate::bindings::exports::#root_ns::#root_name::#stub_interface_name::#interface_name {
                        crate::bindings::exports::#root_ns::#root_name::#stub_interface_name::#interface_name::new(
                            Self {
                                rpc: WasmRpc::ephemeral(component_id)
                            }
                        )
                    }
                }
            } else {
                quote! {
                    fn new(worker_name: String) -> Self {
                        let component_name = #component_name;
                        let worker_id = golem_rust::bindings::golem::api::host::resolve_worker_id(component_name, &worker_name).expect(
                            &format!("Failed to resolve worker id: {}/{}", component_name, worker_name)
                        );
                        Self {
                            rpc: WasmRpc::new(&worker_id)
                        }
                    }

                    fn custom(worker_id: golem_rust::wasm_rpc::WorkerId) -> crate::bindings::exports::#root_ns::#root_name::#stub_interface_name::#interface_name {
                        crate::bindings::exports::#root_ns::#root_name::#stub_interface_name::#interface_name::new(
                            Self {
                                rpc: WasmRpc::new(&worker_id)
                            }
                        )
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

        if entity.is_resource() {
            let remote_function_name = get_remote_function_name(entity, "drop");

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

        use golem_rust::wasm_rpc::*;

        #[allow(dead_code)]
        mod bindings;

        #(#struct_defs)*

        #(#interface_impls)*

        #(#exports)*
    };

    let syntax_tree = syn::parse2(lib)?;
    let src = prettyplease::unparse(&syntax_tree);

    let target_rust_path = PathExtra::new(def.client_rust_path());

    log_action(
        "Generating",
        format!("stub source to {}", target_rust_path.log_color_highlight()),
    );
    println!("target: {:?}", target_rust_path.as_path());
    fs::create_dir_all(target_rust_path.parent()?)?;
    fs::write(def.client_rust_path(), src)?;
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FunctionMode {
    Global,
    Static,
    Method,
    Constructor,
    CustomConstructor,
}

fn generate_result_wrapper_get_source(
    def: &StubDefinition,
    entity: &StubbedEntity,
    function: &FunctionStub,
    interface_name: &Ident,
    mode: FunctionMode,
) -> anyhow::Result<TokenStream> {
    let result_type = get_result_type_source(def, function, interface_name, mode)?;
    let output_values = get_output_values_source(def, function, interface_name, mode)?;

    let remote_function_name = get_remote_function_name(entity, &function.name);

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
    owner: &StubbedEntity,
    interface_name: &Ident,
    mode: FunctionMode,
) -> anyhow::Result<TokenStream> {
    let function_name = Ident::new(&to_rust_ident(&function.name), Span::call_site());
    let mut params = Vec::new();
    let mut param_names = HashSet::new();
    let mut input_values = Vec::new();

    if mode != FunctionMode::Static
        && mode != FunctionMode::Constructor
        && mode != FunctionMode::CustomConstructor
    {
        params.push(quote! {&self});
    }

    if mode == FunctionMode::Constructor && !def.config.is_ephemeral {
        params.push(quote! { wasm_rpc_worker_name: String });
    } else if mode == FunctionMode::CustomConstructor {
        if def.config.is_ephemeral {
            params.push(quote! { wasm_rpc_component_id: golem_rust::wasm_rpc::ComponentId });
        } else {
            params.push(quote! { wasm_rpc_worker_id: golem_rust::wasm_rpc::WorkerId });
        }
    } else if mode == FunctionMode::Method {
        input_values.push(quote! {
            WitValue::builder().handle(self.uri.clone(), self.id)
        });
    }

    for param in &function.params {
        let rust_param_name = to_rust_ident(&param.name);
        let param_name = Ident::new(&rust_param_name, Span::call_site());
        let param_typ = type_to_rust_ident(&param.typ, def)?;
        params.push(quote! {
            #param_name: #param_typ
        });
        param_names.insert(rust_param_name);

        let param_name_access = quote! { #param_name };

        input_values.push(wit_value_builder(
            &param.typ,
            &param_name_access,
            def,
            quote! { WitValue::builder() },
            false,
        )?);
    }

    let result_type = get_result_type_source(def, function, interface_name, mode)?;
    let output_values = get_output_values_source(def, function, interface_name, mode)?;

    let remote_function_name = get_remote_function_name(
        owner,
        if mode == FunctionMode::CustomConstructor {
            "new" // custom constructors still have to call the real remote constructor
        } else {
            &function.name
        },
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
        FunctionMode::Constructor | FunctionMode::CustomConstructor => {
            quote! { rpc }
        }
        _ => {
            quote! { self.rpc }
        }
    };

    let component_name = def.config.component_name.as_str();
    let init = if mode == FunctionMode::Constructor {
        if def.config.is_ephemeral {
            quote! {
                let component_name = #component_name;
                let component_id = golem_rust::bindings::golem::api::host::resolve_component_id(component_name).expect(
                    &format!("Failed to resolve component id: {}", component_name)
                );
                let rpc = WasmRpc::ephemeral(component_id);
            }
        } else {
            quote! {
                let component_name = #component_name;
                let worker_id = golem_rust::bindings::golem::api::host::resolve_worker_id(component_name, &wasm_rpc_worker_name).expect(
                    &format!("Failed to resolve worker id: {}/{}", component_name, wasm_rpc_worker_name)
                );
                let rpc = WasmRpc::new(&worker_id);
            }
        }
    } else if mode == FunctionMode::CustomConstructor {
        if def.config.is_ephemeral {
            quote! {
                let rpc = WasmRpc::ephemeral(wasm_rpc_component_id);
            }
        } else {
            quote! {
                let rpc = WasmRpc::new(&wasm_rpc_worker_id);
            }
        }
    } else {
        quote! {}
    };

    let blocking = {
        let blocking_function_name =
            if mode == FunctionMode::Constructor || mode == FunctionMode::CustomConstructor {
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

    let non_blocking = if mode != FunctionMode::Constructor
        && mode != FunctionMode::CustomConstructor
    {
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
            let root_ns = def.rust_root_namespace();
            let root_name = def.rust_client_root_name();
            let stub_interface_name = def.rust_client_interface_name();
            let result_wrapper = naming::rust::result_wrapper_ident(function, owner);
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

    let scheduled = if mode != FunctionMode::Constructor && mode != FunctionMode::CustomConstructor
    {
        let scheduled_function_name = format!("schedule-{}", function.name);
        let function_name = Ident::new(&to_rust_ident(&scheduled_function_name), Span::call_site());

        let schedule_for_param_name = new_param_name("schedule_for", &mut param_names);
        let schedule_for_param = Ident::new(&schedule_for_param_name, Span::call_site());

        quote! {
            fn #function_name(
                #(#params),*,
                #schedule_for_param: golem_rust::wasm_rpc::wasi::clocks::wall_clock::Datetime
            ) -> golem_rust::wasm_rpc::golem_rpc_0_2_x::types::CancellationToken {
                #init
                #rpc.schedule_cancelable_invocation(
                    #schedule_for_param,
                    #remote_function_name,
                    &[
                        #(#input_values),*
                    ],
                )
            }
        }
    } else {
        quote! {}
    };

    Ok(quote! {
        #blocking
        #non_blocking
        #scheduled
    })
}

fn get_output_values_source(
    def: &StubDefinition,
    function: &FunctionStub,
    interface_name: &Ident,
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
        FunctionResultStub::Unit => {}
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
        FunctionResultStub::SelfType if mode == FunctionMode::CustomConstructor => {
            let root_ns = def.rust_root_namespace();
            let root_name = def.rust_client_root_name();
            let stub_interface_name = def.rust_client_interface_name();

            output_values.push(quote! {
                {
                    let (uri, id) = result.tuple_element(0).expect("tuple not found").handle().expect("handle not found");
                    crate::bindings::exports::#root_ns::#root_name::#stub_interface_name::#interface_name::new(
                        Self {
                            rpc,
                            id,
                            uri
                        }
                    )
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

fn new_param_name(name: &str, used_names: &mut HashSet<String>) -> String {
    let unique_name = if !used_names.contains(name) {
        name.to_string()
    } else {
        let mut counter = 1;
        let mut make_candidate = || {
            counter += 1;
            format!("{name}_{counter}")
        };

        let mut candiate = make_candidate();
        while used_names.contains(&candiate) {
            candiate = make_candidate();
        }

        candiate
    };

    used_names.insert(unique_name.clone());

    unique_name
}

fn get_result_type_source(
    def: &StubDefinition,
    function: &FunctionStub,
    interface_name: &Ident,
    function_mode: FunctionMode,
) -> anyhow::Result<TokenStream> {
    let result_type = match &function.results {
        FunctionResultStub::Anon(typ) => {
            let typ = type_to_rust_ident(typ, def)?;
            quote! {
                #typ
            }
        }
        FunctionResultStub::Unit => {
            quote! {
                ()
            }
        }
        FunctionResultStub::SelfType => {
            if function_mode == FunctionMode::CustomConstructor {
                let root_ns = def.rust_root_namespace();
                let root_name = def.rust_client_root_name();
                let stub_interface_name = def.rust_client_interface_name();

                quote! { crate::bindings::exports::#root_ns::#root_name::#stub_interface_name::#interface_name }
            } else {
                quote! { Self }
            }
        }
    };
    Ok(result_type)
}

fn get_remote_function_name(entity: &StubbedEntity, function_name: &str) -> String {
    match entity {
        StubbedEntity::WorldFunctions(_) => function_name.to_string(),
        StubbedEntity::Interface(_) => match entity.interface_name() {
            Some(interface_name) => format!("{interface_name}.{{{function_name}}}"),
            None => function_name.to_string(),
        },
        StubbedEntity::Resource(inner) => match &inner.owner_interface {
            Some(owner) => format!("{}.{{{}.{}}}", owner, inner.name, function_name),
            None => format!("{}.{}", inner.name, function_name),
        },
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

                    let root_ns = def.rust_root_namespace();
                    let root_name = def.rust_client_root_name();
                    let stub_interface_name = def.rust_client_interface_name();

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

                            if let Some(module_path) = def
                                .client_binding_mapping
                                .get_mapped_module_path(&package.name, interface_name)
                            {
                                path.clear();
                                for entry in module_path {
                                    let ident = Ident::new(&entry, Span::call_site());
                                    path.push(quote! { #ident });
                                }
                            } else {
                                let ns_ident = Ident::new(
                                    &to_rust_ident(&package.name.namespace),
                                    Span::call_site(),
                                );
                                let name_ident = Ident::new(
                                    &to_rust_ident(&package.name.name),
                                    Span::call_site(),
                                );
                                let interface_ident =
                                    Ident::new(&to_rust_ident(interface_name), Span::call_site());
                                path.push(quote! { #ns_ident });
                                path.push(quote! { #name_ident });
                                path.push(quote! { #interface_ident });
                            }
                        }
                        TypeOwner::None => {}
                    }
                    Ok(quote! { #(#path)::*::#typ })
                }
            }
        }
        Type::ErrorContext => Err(anyhow!("ErrorContext is not supported yet")),
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
                TypeDefKind::FixedSizeList(elem, _) => {
                    wit_list_value_builder(elem, name, def, builder_expr)
                }
                TypeDefKind::Future(_) => Ok(quote!(todo!("future"))),
                TypeDefKind::Stream(_) => Ok(quote!(todo!("stream"))),
                TypeDefKind::Type(typ) => wit_value_builder(typ, name, def, builder_expr, is_reference),
                TypeDefKind::Unknown => Ok(quote!(todo!("unknown"))),
            }
        }
        Type::ErrorContext => Err(anyhow!("ErrorContext is not supported yet")),
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
                TypeDefKind::FixedSizeList(elem, _) => extract_from_list_value(elem, def, base_expr),
                TypeDefKind::Future(_) => Ok(quote!(panic!("Future is not supported yet"))),
                TypeDefKind::Stream(_) => Ok(quote!(panic!("Stream is not supported yet"))),
                TypeDefKind::Type(typ) => extract_from_wit_value(typ, def, base_expr),
                TypeDefKind::Unknown => Ok(quote!(panic!("Unexpected unknown type!"))),
            }
        }
        Type::ErrorContext => Err(anyhow!("ErrorContext is not supported yet")),
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
    let root_ns = def.rust_root_namespace();
    let root_name = def.rust_client_root_name();
    let stub_interface_name = def.rust_client_interface_name();

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

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct BindingMappingKey {
    package_name: PackageName,
    interface_name: String,
}

#[derive(Debug, Clone)]
struct BindingMappingEntry {
    module_path: Vec<String>,
}

pub struct BindingMapping {
    mappings: HashMap<BindingMappingKey, BindingMappingEntry>,
}

impl BindingMapping {
    pub fn add_to_cargo_bindings_table(&self, target: &mut HashMap<String, String>) {
        for (key, entry) in &self.mappings {
            let string_key = key.package_name.interface_id(&key.interface_name);
            let string_value = entry.module_path.join("::");
            target.insert(string_key, string_value);
        }
    }

    pub fn get_mapped_module_path(
        &self,
        package_name: &PackageName,
        interface_name: &str,
    ) -> Option<Vec<String>> {
        let key = BindingMappingKey {
            package_name: package_name.clone(),
            interface_name: interface_name.to_string(),
        };
        self.mappings
            .get(&key)
            .map(|value| value.module_path.clone())
    }
}

impl Default for BindingMapping {
    fn default() -> Self {
        let mut mappings = HashMap::new();

        mappings.insert(
            BindingMappingKey {
                package_name: PackageName {
                    namespace: "wasi".to_string(),
                    name: "io".to_string(),
                    version: Some(Version::parse(WASI_WIT_VERSION).unwrap()),
                },
                interface_name: "poll".to_string(),
            },
            BindingMappingEntry {
                module_path: vec![
                    "golem_rust".to_string(),
                    "wasm_rpc".to_string(),
                    "wasi".to_string(),
                    "io".to_string(),
                    "poll".to_string(),
                ],
            },
        );
        mappings.insert(
            BindingMappingKey {
                package_name: PackageName {
                    namespace: "wasi".to_string(),
                    name: "clocks".to_string(),
                    version: Some(Version::parse(WASI_WIT_VERSION).unwrap()),
                },
                interface_name: "wall-clock".to_string(),
            },
            BindingMappingEntry {
                module_path: vec![
                    "golem_rust".to_string(),
                    "wasm_rpc".to_string(),
                    "wasi".to_string(),
                    "clocks".to_string(),
                    "wall_clock".to_string(),
                ],
            },
        );
        mappings.insert(
            BindingMappingKey {
                package_name: PackageName {
                    namespace: "golem".to_string(),
                    name: "rpc".to_string(),
                    version: Some(Version::parse(GOLEM_RPC_WIT_VERSION).unwrap()),
                },
                interface_name: "types".to_string(),
            },
            BindingMappingEntry {
                module_path: vec![
                    "golem_rust".to_string(),
                    "wasm_rpc".to_string(),
                    "golem_rpc_0_2_x".to_string(),
                    "types".to_string(),
                ],
            },
        );

        BindingMapping { mappings }
    }
}
