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

use super::{RustBridgeGenerator, RustInput, RustOutput, RustTypeName};
use anyhow::{anyhow, bail};
use golem_common::schema::agent::{InputSchema, OutputSchema};
use golem_common::schema::schema_type::SchemaType;
use golem_common::schema::unstructured::{
    unstructured_binary_restrictions, unstructured_text_restrictions,
};
use proc_macro2::{Ident, Span, TokenStream};
use quote::quote;
use syn::Index;

impl RustBridgeGenerator {
    pub(super) fn guest_wire_visit(
        &mut self,
        value: TokenStream,
        typ: &SchemaType,
        prepare: bool,
        body: Option<&Ident>,
    ) -> anyhow::Result<TokenStream> {
        if unstructured_text_restrictions(self.type_naming.graph(), typ)?.is_some()
            || unstructured_binary_restrictions(self.type_naming.graph(), typ)?.is_some()
        {
            return Ok(quote! {});
        }
        if body.is_none()
            && let Some(RustTypeName::Derived(name)) = self.type_naming.type_name_for_type(typ)
        {
            let function = Ident::new(
                &format!("{}_{name}", if prepare { "prepare" } else { "preflight" }),
                Span::call_site(),
            );
            return Ok(if prepare {
                quote! { #function(#value).await?; }
            } else {
                quote! { #function(#value, __resources)?; }
            });
        }
        Ok(match typ {
            SchemaType::Record { fields, .. } => {
                let mut statements = Vec::new();
                for field in fields {
                    let name = Ident::new(&self.to_rust_ident(&field.name), Span::call_site());
                    statements.push(self.guest_wire_visit(
                        quote! { &(#value).#name },
                        &field.body,
                        prepare,
                        None,
                    )?);
                }
                quote! { #(#statements)* }
            }
            SchemaType::Variant { cases, .. } => {
                let name = body.ok_or_else(|| anyhow!("unnamed variant in wire visitor"))?;
                let mut arms = Vec::new();
                for case in cases {
                    let case_name =
                        Ident::new(&self.to_rust_case_name(&case.name), Span::call_site());
                    if let Some(payload) = &case.payload {
                        let visit =
                            self.guest_wire_visit(quote! { __payload }, payload, prepare, None)?;
                        arms.push(quote! { #name::#case_name(__payload) => { #visit } });
                    } else {
                        arms.push(quote! { #name::#case_name => {} });
                    }
                }
                quote! { match #value { #(#arms)* } }
            }
            SchemaType::Union { spec, .. } => {
                let name = body.ok_or_else(|| anyhow!("unnamed union in wire visitor"))?;
                let mut arms = Vec::new();
                for branch in &spec.branches {
                    let case = Ident::new(&self.to_rust_case_name(&branch.tag), Span::call_site());
                    let visit =
                        self.guest_wire_visit(quote! { __payload }, &branch.body, prepare, None)?;
                    arms.push(quote! { #name::#case(__payload) => { #visit } });
                }
                quote! { match #value { #(#arms)* } }
            }
            SchemaType::Option { inner, .. } => {
                let visit = self.guest_wire_visit(quote! { __payload }, inner, prepare, None)?;
                quote! { if let Some(__payload) = #value { #visit } }
            }
            SchemaType::Result { spec, .. } => {
                let ok = spec
                    .ok
                    .as_deref()
                    .map(|t| self.guest_wire_visit(quote! { __payload }, t, prepare, None))
                    .transpose()?;
                let err = spec
                    .err
                    .as_deref()
                    .map(|t| self.guest_wire_visit(quote! { __payload }, t, prepare, None))
                    .transpose()?;
                quote! { match #value { Ok(__payload) => { #ok } Err(__payload) => { #err } } }
            }
            SchemaType::List { element, .. } | SchemaType::FixedList { element, .. } => {
                let visit = self.guest_wire_visit(quote! { __item }, element, prepare, None)?;
                let check = if !prepare && let SchemaType::FixedList { length, .. } = typ {
                    let length = *length as usize;
                    quote! {
                        if (#value).len() != #length {
                            return Err(format!("Expected fixed-list of length {}, got {}", #length, (#value).len()));
                        }
                    }
                } else {
                    quote! {}
                };
                quote! { #check for __item in (#value).iter() { #visit } }
            }
            SchemaType::Map {
                key, value: item, ..
            } => {
                let k = self.guest_wire_visit(quote! { __key }, key, prepare, None)?;
                let v = self.guest_wire_visit(quote! { __item }, item, prepare, None)?;
                quote! { for (__key, __item) in (#value).iter() { #k #v } }
            }
            SchemaType::Tuple { elements, .. } => {
                let mut statements = Vec::new();
                for (index, element) in elements.iter().enumerate() {
                    let index = Index::from(index);
                    statements.push(self.guest_wire_visit(
                        quote! { &(#value).#index },
                        element,
                        prepare,
                        None,
                    )?);
                }
                quote! { #(#statements)* }
            }
            SchemaType::Secret { .. }
            | SchemaType::QuotaToken { .. }
            | SchemaType::PermissionCard { .. }
            | SchemaType::Stream { .. } => {
                if prepare {
                    quote! { golem_rust::IntoWire::prepare_wire(#value).await.map_err(|e| e.to_string())?; }
                } else {
                    quote! { golem_rust::IntoWire::preflight(#value, __resources).map_err(|e| e.to_string())?; }
                }
            }
            SchemaType::Datetime { .. } if !prepare => quote! {
                chrono::DateTime::parse_from_rfc3339(#value)
                    .map_err(|__error| format!("Invalid RFC3339 datetime: {__error}"))?;
            },
            _ => quote! {},
        })
    }

    pub(super) fn guest_wire_visitors(
        &mut self,
        name: &Ident,
        typ: &SchemaType,
    ) -> anyhow::Result<TokenStream> {
        let preflight_name = Ident::new(&format!("preflight_{name}"), Span::call_site());
        let prepare_name = Ident::new(&format!("prepare_{name}"), Span::call_site());
        let preflight = self.guest_wire_visit(quote! { value }, typ, false, Some(name))?;
        let prepare = self.guest_wire_visit(quote! { value }, typ, true, Some(name))?;
        Ok(quote! {
            fn #preflight_name(value: &#name, __resources: &mut golem_rust::schema::wit::direct::WirePreflight) -> Result<(), String> {
                #preflight
                Ok(())
            }
            fn #prepare_name(value: &#name) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + '_>> {
                Box::pin(async move { #prepare Ok(()) })
            }
        })
    }

    pub(super) fn guest_input_wire(
        &mut self,
        input: &InputSchema,
        names: &[String],
        asynchronous: bool,
    ) -> anyhow::Result<TokenStream> {
        let mut preflight = Vec::new();
        let mut prepare = Vec::new();
        let mut fields = Vec::new();
        match self.rust_input(input)? {
            RustInput::Params(params) => {
                for ((_, typ), name) in params.iter().zip(names) {
                    let name = Self::ident_from_name(name);
                    preflight.push(self.guest_wire_visit(quote! { &#name }, typ, false, None)?);
                    if asynchronous {
                        prepare.push(self.guest_wire_visit(quote! { &#name }, typ, true, None)?);
                    }
                    let encode = self.guest_wire_encode_expr(quote! { &#name }, typ, false, 0)?;
                    fields.push(quote! { #encode? });
                }
            }
            RustInput::Multimodal(cases) => {
                let name = self.get_or_create_multimodal(&cases);
                let encode = Self::ident_from_name(format!("encode_{name}"));
                let preflight_fn = Self::ident_from_name(format!("preflight_{name}"));
                let prepare_fn = Self::ident_from_name(format!("prepare_{name}"));
                let parameter = Self::ident_from_name(&names[0]);
                preflight.push(
                    quote! { for value in &#parameter { #preflight_fn(value, __resources)?; } },
                );
                if asynchronous {
                    prepare
                        .push(quote! { for value in &#parameter { #prepare_fn(value).await?; } });
                }
                fields.push(quote! {{
                    let items = #parameter.iter().map(|value| #encode(value, __writer)).collect::<Result<Vec<_>, String>>()?;
                    __writer.push(__wire::SchemaValueNode::ListValue(items))
                }});
            }
        }
        let resources = if asynchronous {
            quote! { golem_rust::schema::wit::direct::WirePreflight::asynchronous() }
        } else {
            quote! { golem_rust::schema::wit::direct::WirePreflight::default() }
        };
        let body = quote! {
            let __resources = &mut #resources;
            #(#preflight)*
            #(#prepare)*
            let mut writer = golem_rust::schema::wit::direct::WireWriter::default();
            let __writer = &mut writer;
            let fields = vec![#(#fields),*];
            let root = __writer.push(__wire::SchemaValueNode::RecordValue(fields));
            Ok::<_, String>(writer.finish(root))
        };
        let invoke = if asynchronous {
            quote! { (async move { #body }).await }
        } else {
            quote! { (|| { #body })() }
        };
        Ok(
            quote! { #invoke.map_err(|message| crate::__golem_bridge_runtime::ClientError::SchemaEncodeFailed { message })? },
        )
    }

    pub(super) fn guest_output_wire(
        &mut self,
        output: &OutputSchema,
    ) -> anyhow::Result<TokenStream> {
        let decode = match self.rust_output(output)? {
            RustOutput::Unit => quote! { Ok::<_, String>(()) },
            RustOutput::Single(typ) => {
                self.guest_wire_decode_expr(quote! { __root }, &typ, false, 0)?
            }
            RustOutput::Multimodal(cases) => {
                let name = self.get_or_create_multimodal(&cases);
                let decode = Self::ident_from_name(format!("decode_{name}"));
                quote! { match __reader.take(__root).map_err(|e| e.to_string())? {
                    __wire::SchemaValueNode::ListValue(items) => items.into_iter().map(|index| #decode(index, __reader)).collect::<Result<Vec<_>, String>>(),
                    _ => Err("Expected multimodal list".to_string()),
                } }
            }
        };
        Ok(quote! {
            let __root = __value.root;
            let mut reader = golem_rust::schema::wit::direct::WireReader::new(__value.value_nodes);
            let __reader = &mut reader;
            let value = #decode?;
            reader.finish().map_err(|e| e.to_string())?;
            Ok(value)
        })
    }

    pub(super) fn guest_wire_tree(
        &mut self,
        value: TokenStream,
        typ: &SchemaType,
        asynchronous: bool,
    ) -> anyhow::Result<TokenStream> {
        let preflight = self.guest_wire_visit(quote! { &__item }, typ, false, None)?;
        let prepare = if asynchronous {
            self.guest_wire_visit(quote! { &__item }, typ, true, None)?
        } else {
            quote! {}
        };
        let encode = self.guest_wire_encode_expr(quote! { &__item }, typ, false, 0)?;
        let resources = if asynchronous {
            quote! { golem_rust::schema::wit::direct::WirePreflight::asynchronous() }
        } else {
            quote! { golem_rust::schema::wit::direct::WirePreflight::default() }
        };
        let body = quote! {
            let __item = #value;
            let __resources = &mut #resources;
            #preflight
            #prepare
            let mut writer = golem_rust::schema::wit::direct::WireWriter::default();
            let __writer = &mut writer;
            let root = #encode?;
            Ok::<_, String>(writer.finish(root))
        };
        Ok(if asynchronous {
            quote! { (async move { #body }).await }
        } else {
            quote! { (|| { #body })() }
        })
    }

    pub(super) fn guest_wire_encode_expr(
        &mut self,
        val: TokenStream,
        typ: &SchemaType,
        box_recursive: bool,
        depth: usize,
    ) -> anyhow::Result<TokenStream> {
        let inner = if let Some(name) = self.type_naming.type_name_for_type(typ).cloned() {
            let RustTypeName::Derived(name) = name else {
                bail!("Remapped type names are not supported yet");
            };
            let function = Ident::new(&format!("encode_{name}"), Span::call_site());
            quote! { #function(#val, __writer) }
        } else {
            let inner =
                self.guest_wire_encode_structural(quote! { __source }, typ, box_recursive, depth)?;
            quote! { { let __source = #val; #inner } }
        };
        Ok(quote! {{ let __result: Result<i32, String> = { #inner }; __result }})
    }

    pub(super) fn guest_wire_decode_expr(
        &mut self,
        index: TokenStream,
        typ: &SchemaType,
        box_recursive: bool,
        depth: usize,
    ) -> anyhow::Result<TokenStream> {
        let inner = if let Some(name) = self.type_naming.type_name_for_type(typ).cloned() {
            let RustTypeName::Derived(name) = name else {
                bail!("Remapped type names are not supported yet");
            };
            let function = Ident::new(&format!("decode_{name}"), Span::call_site());
            if box_recursive && self.type_naming.is_recursive_ref(typ) {
                quote! { #function(#index, __reader).map(Box::new) }
            } else {
                quote! { #function(#index, __reader) }
            }
        } else {
            self.guest_wire_decode_structural(index, typ, box_recursive, depth)?
        };
        Ok(quote! {{ let __result: Result<_, String> = { #inner }; __result }})
    }

    fn guest_wire_encode_structural(
        &mut self,
        val: TokenStream,
        typ: &SchemaType,
        box_recursive: bool,
        depth: usize,
    ) -> anyhow::Result<TokenStream> {
        let text = unstructured_text_restrictions(self.type_naming.graph(), typ)?.cloned();
        if let Some(restrictions) = text {
            let ty = self.unstructured_text_type(&restrictions);
            return Ok(quote! {
                <#ty as golem_rust::schema::wit::direct::IntoWire>::write_wire(#val, __writer)
                    .map_err(|__error| __error.to_string())
            });
        }
        let binary = unstructured_binary_restrictions(self.type_naming.graph(), typ)?.cloned();
        if let Some(restrictions) = binary {
            let ty = self.unstructured_binary_type(&restrictions);
            return Ok(quote! {
                <#ty as golem_rust::schema::wit::direct::IntoWire>::write_wire(#val, __writer)
                    .map_err(|__error| __error.to_string())
            });
        }

        let element = Ident::new(&format!("__element{depth}"), Span::call_site());
        let next = depth + 1;
        let push = |node: TokenStream| quote! { Ok(__writer.push(#node)) };
        Ok(match typ {
            SchemaType::Bool { .. } => push(quote! { __wire::SchemaValueNode::BoolValue(*#val) }),
            SchemaType::S8 { .. } => push(quote! { __wire::SchemaValueNode::S8Value(*#val) }),
            SchemaType::S16 { .. } => push(quote! { __wire::SchemaValueNode::S16Value(*#val) }),
            SchemaType::S32 { .. } => push(quote! { __wire::SchemaValueNode::S32Value(*#val) }),
            SchemaType::S64 { .. } => push(quote! { __wire::SchemaValueNode::S64Value(*#val) }),
            SchemaType::U8 { .. } => push(quote! { __wire::SchemaValueNode::U8Value(*#val) }),
            SchemaType::U16 { .. } => push(quote! { __wire::SchemaValueNode::U16Value(*#val) }),
            SchemaType::U32 { .. } => push(quote! { __wire::SchemaValueNode::U32Value(*#val) }),
            SchemaType::U64 { .. } => push(quote! { __wire::SchemaValueNode::U64Value(*#val) }),
            SchemaType::F32 { .. } => push(quote! { __wire::SchemaValueNode::F32Value(*#val) }),
            SchemaType::F64 { .. } => push(quote! { __wire::SchemaValueNode::F64Value(*#val) }),
            SchemaType::Char { .. } => push(quote! { __wire::SchemaValueNode::CharValue(*#val) }),
            SchemaType::String { .. } => {
                push(quote! { __wire::SchemaValueNode::StringValue(#val.clone()) })
            }
            SchemaType::Path { .. } => {
                push(quote! { __wire::SchemaValueNode::PathValue(#val.clone()) })
            }
            SchemaType::Url { .. } => {
                push(quote! { __wire::SchemaValueNode::UrlValue(#val.clone()) })
            }
            SchemaType::Duration { .. } => push(quote! {
                __wire::SchemaValueNode::DurationValue(__wire::DurationValuePayload { nanoseconds: *#val })
            }),
            SchemaType::Datetime { .. } => quote! {
                chrono::DateTime::parse_from_rfc3339(&#val)
                    .map_err(|__error| format!("Invalid RFC3339 datetime: {__error}"))
                    .map(|__datetime| __writer.push(__wire::SchemaValueNode::DatetimeValue(
                        __wire::Datetime {
                            seconds: __datetime.timestamp(),
                            nanoseconds: __datetime.timestamp_subsec_nanos(),
                        }
                    )))
            },
            SchemaType::Option { inner, .. } => {
                let encode =
                    self.guest_wire_encode_expr(quote! { #element }, inner, box_recursive, next)?;
                quote! {
                    match #val {
                        Some(#element) => { let payload = #encode?; Ok(__writer.push(__wire::SchemaValueNode::OptionValue(Some(payload)))) },
                        None => Ok(__writer.push(__wire::SchemaValueNode::OptionValue(None))),
                    }
                }
            }
            SchemaType::List { element: item, .. }
            | SchemaType::FixedList { element: item, .. } => {
                let encode = self.guest_wire_encode_expr(quote! { #element }, item, false, next)?;
                let node = if matches!(typ, SchemaType::List { .. }) {
                    quote! { __wire::SchemaValueNode::ListValue(__indices) }
                } else {
                    quote! { __wire::SchemaValueNode::FixedListValue(__indices) }
                };
                let length_check = if let SchemaType::FixedList { length, .. } = typ {
                    let length = *length as usize;
                    quote! {
                        if #val.len() != #length {
                            return Err(format!("Expected fixed-list of length {}, got {}", #length, #val.len()));
                        }
                    }
                } else {
                    quote! {}
                };
                quote! {{
                    #length_check
                    let __indices = #val.into_iter().map(|#element| #encode).collect::<Result<Vec<_>, String>>()?;
                    Ok(__writer.push(#node))
                }}
            }
            SchemaType::Map { key, value, .. } => {
                let key_name = Ident::new(&format!("__key{depth}"), Span::call_site());
                let value_name = Ident::new(&format!("__value{depth}"), Span::call_site());
                let encode_key =
                    self.guest_wire_encode_expr(quote! { #key_name }, key, false, next)?;
                let encode_value =
                    self.guest_wire_encode_expr(quote! { #value_name }, value, false, next)?;
                quote! {{
                    let __entries = #val.into_iter().map(|(#key_name, #value_name)| {
                        Ok::<_, String>(__wire::MapEntry { key: #encode_key?, value: #encode_value? })
                    }).collect::<Result<Vec<_>, String>>()?;
                    Ok(__writer.push(__wire::SchemaValueNode::MapValue(__entries)))
                }}
            }
            SchemaType::Tuple { elements, .. } => {
                let tuple = Ident::new(&format!("__tuple{depth}"), Span::call_site());
                let mut encoded = Vec::new();
                for (position, item) in elements.iter().enumerate() {
                    let position = Index::from(position);
                    let encode = self.guest_wire_encode_expr(
                        quote! { &#tuple.#position },
                        item,
                        box_recursive,
                        next,
                    )?;
                    encoded.push(quote! { #encode? });
                }
                quote! {{
                    let #tuple = #val;
                    let elements = vec![#(#encoded),*];
                    Ok(__writer.push(__wire::SchemaValueNode::TupleValue(elements)))
                }}
            }
            SchemaType::Result { spec, .. } => {
                let ok = match spec.ok.as_deref() {
                    Some(item) => {
                        let encode = self.guest_wire_encode_expr(
                            quote! { __payload },
                            item,
                            box_recursive,
                            next,
                        )?;
                        quote! { Ok(__payload) => __wire::ResultValuePayload::OkValue(Some(#encode?)), }
                    }
                    None => quote! { Ok(_) => __wire::ResultValuePayload::OkValue(None), },
                };
                let err = match spec.err.as_deref() {
                    Some(item) => {
                        let encode = self.guest_wire_encode_expr(
                            quote! { __payload },
                            item,
                            box_recursive,
                            next,
                        )?;
                        quote! { Err(__payload) => __wire::ResultValuePayload::ErrValue(Some(#encode?)), }
                    }
                    None => quote! { Err(_) => __wire::ResultValuePayload::ErrValue(None), },
                };
                quote! {{
                    let __payload = match #val { #ok #err };
                    Ok(__writer.push(__wire::SchemaValueNode::ResultValue(__payload)))
                }}
            }
            SchemaType::Secret { .. }
            | SchemaType::QuotaToken { .. }
            | SchemaType::PermissionCard { .. }
            | SchemaType::Stream { .. } => quote! {
                golem_rust::schema::wit::direct::IntoWire::write_wire(#val, __writer)
                    .map_err(|__error| __error.to_string())
            },
            SchemaType::Text { .. } => bail!("Bare text rich scalars have no Rust bridge surface"),
            SchemaType::Binary { .. } => {
                bail!("Bare binary rich scalars have no guest Rust bridge surface")
            }
            SchemaType::Ref { .. }
            | SchemaType::Record { .. }
            | SchemaType::Variant { .. }
            | SchemaType::Enum { .. }
            | SchemaType::Flags { .. }
            | SchemaType::Union { .. } => {
                bail!("Expected a generated type name for {typ:?} during wire encoding")
            }
            SchemaType::Quantity { .. } | SchemaType::Future { .. } => {
                bail!("SchemaType variant has no guest wire encoding yet; type = {typ:?}")
            }
        })
    }

    fn guest_wire_decode_structural(
        &mut self,
        index: TokenStream,
        typ: &SchemaType,
        box_recursive: bool,
        depth: usize,
    ) -> anyhow::Result<TokenStream> {
        let text = unstructured_text_restrictions(self.type_naming.graph(), typ)?.cloned();
        if let Some(restrictions) = text {
            let ty = self.unstructured_text_type(&restrictions);
            return Ok(quote! {
                <#ty as golem_rust::schema::wit::direct::FromWire>::read_wire(__reader, #index)
                    .map_err(|__error| __error.to_string())
            });
        }
        let binary = unstructured_binary_restrictions(self.type_naming.graph(), typ)?.cloned();
        if let Some(restrictions) = binary {
            let ty = self.unstructured_binary_type(&restrictions);
            return Ok(quote! {
                <#ty as golem_rust::schema::wit::direct::FromWire>::read_wire(__reader, #index)
                    .map_err(|__error| __error.to_string())
            });
        }

        let next = depth + 1;
        macro_rules! scalar {
            ($variant:ident, $expected:literal) => {
                quote! { match __reader.take(#index).map_err(|e| e.to_string())? {
                    __wire::SchemaValueNode::$variant(__value) => Ok(__value),
                    __other => Err(format!(concat!("Expected ", $expected, " value, got {:?}"), __other)),
                }}
            };
        }
        Ok(match typ {
            SchemaType::Bool { .. } => scalar!(BoolValue, "bool"),
            SchemaType::S8 { .. } => scalar!(S8Value, "s8"),
            SchemaType::S16 { .. } => scalar!(S16Value, "s16"),
            SchemaType::S32 { .. } => scalar!(S32Value, "s32"),
            SchemaType::S64 { .. } => scalar!(S64Value, "s64"),
            SchemaType::U8 { .. } => scalar!(U8Value, "u8"),
            SchemaType::U16 { .. } => scalar!(U16Value, "u16"),
            SchemaType::U32 { .. } => scalar!(U32Value, "u32"),
            SchemaType::U64 { .. } => scalar!(U64Value, "u64"),
            SchemaType::F32 { .. } => scalar!(F32Value, "f32"),
            SchemaType::F64 { .. } => scalar!(F64Value, "f64"),
            SchemaType::Char { .. } => scalar!(CharValue, "char"),
            SchemaType::String { .. } => scalar!(StringValue, "string"),
            SchemaType::Path { .. } => scalar!(PathValue, "path"),
            SchemaType::Url { .. } => scalar!(UrlValue, "url"),
            SchemaType::Duration { .. } => {
                quote! { match __reader.take(#index).map_err(|e| e.to_string())? {
                    __wire::SchemaValueNode::DurationValue(__value) => Ok(__value.nanoseconds),
                    __other => Err(format!("Expected duration value, got {:?}", __other)),
                }}
            }
            SchemaType::Datetime { .. } => {
                quote! { match __reader.take(#index).map_err(|e| e.to_string())? {
                    __wire::SchemaValueNode::DatetimeValue(__value) =>
                        chrono::DateTime::<chrono::Utc>::from_timestamp(__value.seconds, __value.nanoseconds)
                            .map(|__value| __value.to_rfc3339())
                            .ok_or_else(|| "Expected valid datetime value".to_string()),
                    __other => Err(format!("Expected datetime value, got {:?}", __other)),
                }}
            }
            SchemaType::Option { inner, .. } => {
                let decode =
                    self.guest_wire_decode_expr(quote! { __inner }, inner, box_recursive, next)?;
                quote! { match __reader.take(#index).map_err(|e| e.to_string())? {
                    __wire::SchemaValueNode::OptionValue(Some(__inner)) => Ok(Some(#decode?)),
                    __wire::SchemaValueNode::OptionValue(None) => Ok(None),
                    __other => Err(format!("Expected option value, got {:?}", __other)),
                }}
            }
            SchemaType::List { element, .. } | SchemaType::FixedList { element, .. } => {
                let decode =
                    self.guest_wire_decode_expr(quote! { __child }, element, false, next)?;
                let pattern = if matches!(typ, SchemaType::List { .. }) {
                    quote! { __wire::SchemaValueNode::ListValue(__children) }
                } else {
                    quote! { __wire::SchemaValueNode::FixedListValue(__children) }
                };
                let expected = if matches!(typ, SchemaType::List { .. }) {
                    "list"
                } else {
                    "fixed-list"
                };
                let check = if let SchemaType::FixedList { length, .. } = typ {
                    let length = *length as usize;
                    quote! { if __children.len() != #length { return Err(format!("Expected fixed-list of length {}, got {}", #length, __children.len())); } }
                } else {
                    quote! {}
                };
                quote! { match __reader.take(#index).map_err(|e| e.to_string())? {
                    #pattern => { #check __children.into_iter().map(|__child| #decode).collect::<Result<Vec<_>, String>>() }
                    __other => Err(format!("Expected {} value, got {:?}", #expected, __other)),
                }}
            }
            SchemaType::Map { key, value, .. } => {
                let decode_key =
                    self.guest_wire_decode_expr(quote! { __entry.key }, key, false, next)?;
                let decode_value =
                    self.guest_wire_decode_expr(quote! { __entry.value }, value, false, next)?;
                quote! { match __reader.take(#index).map_err(|e| e.to_string())? {
                    __wire::SchemaValueNode::MapValue(__entries) => __entries.into_iter().map(|__entry| Ok::<_, String>((#decode_key?, #decode_value?))).collect(),
                    __other => Err(format!("Expected map value, got {:?}", __other)),
                }}
            }
            SchemaType::Tuple { elements, .. } => {
                let count = elements.len();
                let mut decoded = Vec::new();
                for item in elements {
                    let decode = self.guest_wire_decode_expr(
                        quote! { __children.next().unwrap() },
                        item,
                        box_recursive,
                        next,
                    )?;
                    decoded.push(quote! { #decode? });
                }
                let tuple = if count == 0 {
                    quote! { () }
                } else if count == 1 {
                    quote! { (#(#decoded),*,) }
                } else {
                    quote! { (#(#decoded),*) }
                };
                quote! { match __reader.take(#index).map_err(|e| e.to_string())? {
                    __wire::SchemaValueNode::TupleValue(__children) => {
                        if __children.len() != #count { return Err(format!("Expected tuple with {} elements, got {}", #count, __children.len())); }
                        let mut __children = __children.into_iter(); Ok(#tuple)
                    }
                    __other => Err(format!("Expected tuple value, got {:?}", __other)),
                }}
            }
            SchemaType::Result { spec, .. } => {
                let ok = match spec.ok.as_deref() {
                    Some(item) => {
                        let dec = self.guest_wire_decode_expr(
                            quote! { __payload },
                            item,
                            box_recursive,
                            next,
                        )?;
                        quote! { Some(__payload) => Ok(Ok(#dec?)), None => Err("Missing ok value".to_string()), }
                    }
                    None => {
                        quote! { None => Ok(Ok(())), Some(_) => Err("Unexpected ok value".to_string()), }
                    }
                };
                let err = match spec.err.as_deref() {
                    Some(item) => {
                        let dec = self.guest_wire_decode_expr(
                            quote! { __payload },
                            item,
                            box_recursive,
                            next,
                        )?;
                        quote! { Some(__payload) => Ok(Err(#dec?)), None => Err("Missing err value".to_string()), }
                    }
                    None => {
                        quote! { None => Ok(Err(())), Some(_) => Err("Unexpected err value".to_string()), }
                    }
                };
                quote! { match __reader.take(#index).map_err(|e| e.to_string())? {
                    __wire::SchemaValueNode::ResultValue(__wire::ResultValuePayload::OkValue(__payload)) => match __payload { #ok },
                    __wire::SchemaValueNode::ResultValue(__wire::ResultValuePayload::ErrValue(__payload)) => match __payload { #err },
                    __other => Err(format!("Expected result value, got {:?}", __other)),
                }}
            }
            SchemaType::Secret { .. } => {
                quote! { <golem_rust::secrets::GuestSecretHandle as golem_rust::schema::wit::direct::FromWire>::read_wire(__reader, #index).map_err(|e| e.to_string()) }
            }
            SchemaType::QuotaToken { .. } => {
                quote! { <golem_rust::quota::QuotaToken as golem_rust::schema::wit::direct::FromWire>::read_wire(__reader, #index).map_err(|e| e.to_string()) }
            }
            SchemaType::PermissionCard { .. } => {
                quote! { <golem_rust::schema::wit::GuestPermissionCardHandle as golem_rust::schema::wit::direct::FromWire>::read_wire(__reader, #index).map_err(|e| e.to_string()) }
            }
            SchemaType::Stream { inner, .. } => {
                let inner = inner
                    .as_deref()
                    .ok_or_else(|| anyhow!("Cannot generate an untyped Rust AgentStream"))?;
                let decode = self.guest_wire_decode_expr(quote! { __root }, inner, false, next)?;
                quote! {{
                    let stream = <golem_rust::schema::SchemaValueStream as golem_rust::FromWire>::read_wire(__reader, #index)
                        .map_err(|e| e.to_string())?;
                    Ok(golem_rust::agentic::AgentStream::from_schema_stream_with_wire_decoder(stream, |tree| {
                        let __root = tree.root;
                        let mut reader = golem_rust::schema::wit::direct::WireReader::new(tree.value_nodes);
                        let __reader = &mut reader;
                        let value = #decode?;
                        reader.finish().map_err(|e| e.to_string())?;
                        Ok(value)
                    }))
                }}
            }
            SchemaType::Text { .. } => bail!("Bare text rich scalars have no Rust bridge surface"),
            SchemaType::Binary { .. } => {
                bail!("Bare binary rich scalars have no guest Rust bridge surface")
            }
            SchemaType::Ref { .. }
            | SchemaType::Record { .. }
            | SchemaType::Variant { .. }
            | SchemaType::Enum { .. }
            | SchemaType::Flags { .. }
            | SchemaType::Union { .. } => {
                bail!("Expected a generated type name for {typ:?} during wire decoding")
            }
            SchemaType::Quantity { .. } | SchemaType::Future { .. } => {
                bail!("SchemaType variant has no guest wire decoding yet; type = {typ:?}")
            }
        })
    }

    pub(super) fn guest_wire_encode_body(
        &mut self,
        name: &Ident,
        resolved: &SchemaType,
    ) -> anyhow::Result<TokenStream> {
        match resolved {
            SchemaType::Record { fields, .. } => {
                let names = fields
                    .iter()
                    .map(|field| Ident::new(&self.to_rust_ident(&field.name), Span::call_site()))
                    .collect::<Vec<_>>();
                let mut encoded = Vec::new();
                for (field, field_name) in fields.iter().zip(&names) {
                    let enc =
                        self.guest_wire_encode_expr(quote! { #field_name }, &field.body, true, 0)?;
                    encoded.push(quote! { #enc? });
                }
                Ok(
                    quote! { let #name { #(#names),* } = value; let fields = vec![#(#encoded),*]; Ok(__writer.push(__wire::SchemaValueNode::RecordValue(fields))) },
                )
            }
            SchemaType::Variant { cases, .. } => {
                let mut complete = Vec::new();
                for (case_index, case) in cases.iter().enumerate() {
                    let case_name =
                        Ident::new(&self.to_rust_case_name(&case.name), Span::call_site());
                    let i = case_index as u32;
                    if let Some(payload) = &case.payload {
                        let enc =
                            self.guest_wire_encode_expr(quote! { __payload }, payload, true, 0)?;
                        complete
                            .push(quote! { #name::#case_name(__payload) => (#i, Some(#enc?)), });
                    } else {
                        complete.push(quote! { #name::#case_name => (#i, None), });
                    }
                }
                Ok(
                    quote! { let (__case, __payload) = match value { #(#complete)* }; Ok(__writer.push(__wire::SchemaValueNode::VariantValue(__wire::VariantValuePayload { case: __case, payload: __payload }))) },
                )
            }
            SchemaType::Enum { cases, .. } => {
                let arms = cases.iter().enumerate().map(|(i, case)| {
                    let case = Ident::new(&self.to_rust_case_name(case), Span::call_site());
                    let i = i as u32;
                    quote! { #name::#case => #i, }
                });
                Ok(
                    quote! { Ok(__writer.push(__wire::SchemaValueNode::EnumValue(match value { #(#arms)* }))) },
                )
            }
            SchemaType::Flags { flags, .. } => {
                let names = flags
                    .iter()
                    .map(|flag| Ident::new(&self.to_rust_ident(flag), Span::call_site()))
                    .collect::<Vec<_>>();
                Ok(
                    quote! { let #name { #(#names),* } = value; Ok(__writer.push(__wire::SchemaValueNode::FlagsValue(vec![#(*#names),*]))) },
                )
            }
            SchemaType::Union { spec, .. } => {
                let mut arms = Vec::new();
                for branch in &spec.branches {
                    let case = Ident::new(&self.to_rust_case_name(&branch.tag), Span::call_site());
                    let tag = &branch.tag;
                    let enc =
                        self.guest_wire_encode_expr(quote! { __body }, &branch.body, true, 0)?;
                    arms.push(quote! { #name::#case(__body) => (#tag.to_string(), #enc?), });
                }
                Ok(
                    quote! { let (__tag, __body) = match value { #(#arms)* }; Ok(__writer.push(__wire::SchemaValueNode::UnionValue(__wire::UnionValuePayload { tag: __tag, body: __body }))) },
                )
            }
            other => self.guest_wire_encode_structural(quote! { value }, other, true, 0),
        }
    }

    pub(super) fn guest_wire_decode_body(
        &mut self,
        name: &Ident,
        resolved: &SchemaType,
    ) -> anyhow::Result<TokenStream> {
        match resolved {
            SchemaType::Record { fields, .. } => {
                let count = fields.len();
                let mut values = Vec::new();
                for field in fields {
                    let field_name =
                        Ident::new(&self.to_rust_ident(&field.name), Span::call_site());
                    let dec = self.guest_wire_decode_expr(
                        quote! { __fields.next().unwrap() },
                        &field.body,
                        true,
                        0,
                    )?;
                    values.push(quote! { #field_name: #dec? });
                }
                Ok(
                    quote! { match __reader.take(value).map_err(|e| e.to_string())? { __wire::SchemaValueNode::RecordValue(__fields) => { if __fields.len() != #count { return Err(format!("Expected record with {} fields, got {}", #count, __fields.len())); } let mut __fields = __fields.into_iter(); Ok(#name { #(#values),* }) } __other => Err(format!("Expected record value, got {:?}", __other)), } },
                )
            }
            SchemaType::Variant { cases, .. } => {
                let mut arms = Vec::new();
                for (i, case) in cases.iter().enumerate() {
                    let case_name =
                        Ident::new(&self.to_rust_case_name(&case.name), Span::call_site());
                    let i = i as u32;
                    if let Some(payload) = &case.payload {
                        let dec =
                            self.guest_wire_decode_expr(quote! { __payload }, payload, true, 0)?;
                        arms.push(quote! { #i => { let __payload = __payload.ok_or_else(|| format!("Missing payload for variant case {}", #i))?; Ok(#name::#case_name(#dec?)) } });
                    } else {
                        arms.push(quote! { #i => if __payload.is_none() { Ok(#name::#case_name) } else { Err(format!("Unexpected payload for variant case {}", #i)) }, });
                    }
                }
                Ok(
                    quote! { match __reader.take(value).map_err(|e| e.to_string())? { __wire::SchemaValueNode::VariantValue(__wire::VariantValuePayload { case: __case, payload: __payload }) => match __case { #(#arms)* __other => Err(format!("Invalid variant case index: {}", __other)), }, __other => Err(format!("Expected variant value, got {:?}", __other)), } },
                )
            }
            SchemaType::Enum { cases, .. } => {
                let arms = cases.iter().enumerate().map(|(i, case)| {
                    let case = Ident::new(&self.to_rust_case_name(case), Span::call_site());
                    let i = i as u32;
                    quote! { #i => Ok(#name::#case), }
                });
                Ok(
                    quote! { match __reader.take(value).map_err(|e| e.to_string())? { __wire::SchemaValueNode::EnumValue(__case) => match __case { #(#arms)* __other => Err(format!("Invalid enum case index: {}", __other)), }, __other => Err(format!("Expected enum value, got {:?}", __other)), } },
                )
            }
            SchemaType::Flags { flags, .. } => {
                let count = flags.len();
                let values = flags.iter().enumerate().map(|(i, flag)| {
                    let flag = Ident::new(&self.to_rust_ident(flag), Span::call_site());
                    quote! { #flag: __bits[#i] }
                });
                Ok(
                    quote! { match __reader.take(value).map_err(|e| e.to_string())? { __wire::SchemaValueNode::FlagsValue(__bits) => { if __bits.len() != #count { return Err(format!("Expected flags with {} bits, got {}", #count, __bits.len())); } Ok(#name { #(#values),* }) } __other => Err(format!("Expected flags value, got {:?}", __other)), } },
                )
            }
            SchemaType::Union { spec, .. } => {
                let mut arms = Vec::new();
                for branch in &spec.branches {
                    let case = Ident::new(&self.to_rust_case_name(&branch.tag), Span::call_site());
                    let tag = &branch.tag;
                    let dec =
                        self.guest_wire_decode_expr(quote! { __body }, &branch.body, true, 0)?;
                    arms.push(quote! { #tag => Ok(#name::#case(#dec?)), });
                }
                Ok(
                    quote! { match __reader.take(value).map_err(|e| e.to_string())? { __wire::SchemaValueNode::UnionValue(__wire::UnionValuePayload { tag: __tag, body: __body }) => match __tag.as_str() { #(#arms)* __other => Err(format!("Unknown union branch tag: {}", __other)), }, __other => Err(format!("Expected union value, got {:?}", __other)), } },
                )
            }
            other => self.guest_wire_decode_structural(quote! { value }, other, true, 0),
        }
    }
}
