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

use syn::{GenericArgument, PathArguments, ReturnType, Type};

pub struct FunctionInputInfo {
    pub input_shape: DefaultOrMultimodal,
}

impl FunctionInputInfo {
    pub fn from_signature(sig: &syn::Signature) -> FunctionInputInfo {
        let typed_params: Vec<_> = skip_self_parameters(sig);

        if typed_params.len() == 1 {
            let only_param = &typed_params[0];

            if let Type::Path(type_path) = &*only_param.ty {
                if let Some(seg) = type_path.path.segments.last() {
                    if seg.ident == "Multimodal" {
                        return FunctionInputInfo {
                            input_shape: DefaultOrMultimodal::Multimodal,
                        };
                    }
                }
            }
        }

        FunctionInputInfo {
            input_shape: DefaultOrMultimodal::Default,
        }
    }
}

pub struct FunctionOutputInfo {
    pub output_shape: DefaultOrMultimodal,
    pub future_or_immediate: FutureOrImmediate,
    pub is_unit: bool,
}

impl FunctionOutputInfo {
    pub fn from_signature(sig: &syn::Signature) -> FunctionOutputInfo {
        let function_kind = get_function_kind(sig);

        if let syn::ReturnType::Type(_, ty) = &sig.output {
            if let Some(inner_type) = extract_inner_type_if_future(ty) {
                if is_multimodal_type(inner_type) {
                    return FunctionOutputInfo {
                        output_shape: DefaultOrMultimodal::Multimodal,
                        future_or_immediate: function_kind,
                        is_unit: false,
                    };
                }
            } else if is_multimodal_type(ty) {
                return FunctionOutputInfo {
                    output_shape: DefaultOrMultimodal::Multimodal,
                    future_or_immediate: function_kind,
                    is_unit: false,
                };
            }
        }

        let is_unit = match &sig.output {
            ReturnType::Type(_, ty) => match &**ty {
                Type::Tuple(tuple) => tuple.elems.is_empty(),
                _ => false,
            },
            _ => true,
        };

        FunctionOutputInfo {
            output_shape: DefaultOrMultimodal::Default,
            future_or_immediate: function_kind,
            is_unit,
        }
    }
}

pub enum FutureOrImmediate {
    Future,
    Immediate,
}

// DefaultOrMultimodal refers to the type of input parameters (not each parameter individually) or output parameter type
// Default refers all types that can be part of DataValue::Tuple
pub enum DefaultOrMultimodal {
    Default,
    Multimodal,
}

pub fn is_constructor_method(sig: &syn::Signature) -> bool {
    match &sig.output {
        ReturnType::Type(_, ty) => match &**ty {
            Type::Path(tp) => tp.path.segments.last().unwrap().ident == "Self",
            _ => false,
        },
        _ => false,
    }
}

pub fn get_function_kind(sig: &syn::Signature) -> FutureOrImmediate {
    if sig.asyncness.is_some() {
        FutureOrImmediate::Future
    } else {
        FutureOrImmediate::Immediate
    }
}

pub fn skip_self_parameters(sig: &syn::Signature) -> Vec<&syn::PatType> {
    sig.inputs
        .iter()
        .filter_map(|arg| match arg {
            syn::FnArg::Typed(pat_ty) => Some(pat_ty),
            _ => None,
        })
        .collect()
}

fn extract_inner_type_if_future(ty: &Type) -> Option<&Type> {
    if let Type::Path(type_path) = ty {
        if let Some(seg) = type_path.path.segments.last() {
            if seg.ident == "impl" || seg.ident == "Future" {
                if let PathArguments::AngleBracketed(args) = &seg.arguments {
                    for arg in &args.args {
                        if let GenericArgument::Type(inner_ty) = arg {
                            return Some(inner_ty);
                        }
                    }
                }
            }
        }
    }
    None
}

pub fn extract_inner_type_if_multimodal(ty: &Type) -> Option<&Type> {
    if let Type::Path(type_path) = ty {
        if let Some(segment) = type_path.path.segments.last() {
            if segment.ident == "Multimodal" {
                if let syn::PathArguments::AngleBracketed(args) = &segment.arguments {
                    if let Some(syn::GenericArgument::Type(inner_ty)) = args.args.first() {
                        return Some(inner_ty);
                    }
                }
            }
        }
    }

    None
}

pub fn is_unstructured_text(ty: &Type) -> bool {
    if let Type::Path(type_path) = ty {
        if let Some(seg) = type_path.path.segments.last() {
            return seg.ident == "UnstructuredText";
        }
    }
    false
}

pub fn is_unstructured_binary(ty: &Type) -> bool {
    if let Type::Path(type_path) = ty {
        if let Some(seg) = type_path.path.segments.last() {
            return seg.ident == "UnstructuredBinary";
        }
    }
    false
}

fn is_multimodal_type(ty: &Type) -> bool {
    if let Type::Path(type_path) = ty {
        if let Some(seg) = type_path.path.segments.last() {
            return seg.ident == "Multimodal";
        }
    }
    false
}
