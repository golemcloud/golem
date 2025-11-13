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

pub struct InputParamInfo {
    pub param_type: ParamType,
}

pub struct OutputParamInfo {
    pub param_type: ParamType,
    pub function_kind: FunctionKind,
    pub is_unit: bool,
}

pub enum FunctionKind {
    Async,
    Sync,
}

#[derive(Debug)]
pub enum ParamType {
    Tuple,
    Multimodal,
}

pub fn get_input_param_info(sig: &syn::Signature) -> InputParamInfo {
    let typed_params: Vec<_> = skip_self_parameters(sig);

    if typed_params.len() == 1 {
        let only_param = &typed_params[0];

        if let Type::Path(type_path) = &*only_param.ty {
            if let Some(seg) = type_path.path.segments.last() {
                if seg.ident == "MultiModal" {
                    return InputParamInfo {
                        param_type: ParamType::Multimodal,
                    };
                }
            }
        }
    }

    InputParamInfo {
        param_type: ParamType::Tuple,
    }
}

pub fn get_output_param_info(sig: &syn::Signature) -> OutputParamInfo {
    let function_kind = get_function_kind(sig);

    if let syn::ReturnType::Type(_, ty) = &sig.output {
        if let Some(inner_type) = extract_inner_type_if_future(ty) {
            if is_multimodal_type(inner_type) {
                return OutputParamInfo {
                    param_type: ParamType::Multimodal,
                    function_kind,
                    is_unit: false,
                };
            }
        } else if is_multimodal_type(ty) {
            return OutputParamInfo {
                param_type: ParamType::Multimodal,
                function_kind,
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

    OutputParamInfo {
        param_type: ParamType::Tuple,
        function_kind,
        is_unit,
    }
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

pub fn get_function_kind(sig: &syn::Signature) -> FunctionKind {
    if sig.asyncness.is_some() {
        FunctionKind::Async
    } else {
        FunctionKind::Sync
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
            if segment.ident == "MultiModal" {
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

fn is_multimodal_type(ty: &Type) -> bool {
    if let Type::Path(type_path) = ty {
        println!("type: {:#?}", ty);

        if let Some(seg) = type_path.path.segments.last() {
            return seg.ident == "MultiModal";
        }
    }
    false
}
