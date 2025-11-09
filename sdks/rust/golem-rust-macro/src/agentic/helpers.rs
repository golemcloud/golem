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

pub struct InputParamType {
    pub param_type: ParamType,
}

pub struct OutputParamType {
    pub param_type: ParamType,
    pub function_kind: FunctionKind,
}

pub enum FunctionKind {
    Async,
    Sync,
}

pub enum ParamType {
    Tuple,
    Multimodal,
}

pub fn get_input_param_type(sig: &syn::Signature) -> InputParamType {
    if sig.inputs.len() == 1 {
        if let syn::FnArg::Typed(pat_ty) = &sig.inputs[0] {
            if let syn::Type::Path(type_path) = &*pat_ty.ty {
                if let Some(seg) = type_path.path.segments.last() {
                    if seg.ident == "Multimodal" {
                        // Depends on how exactly multimodal is represented
                        return InputParamType {
                            param_type: ParamType::Multimodal,
                        };
                    }
                }
            }
        }
    }
    InputParamType {
        param_type: ParamType::Tuple,
    }
}

pub fn get_output_param_type(sig: &syn::Signature) -> OutputParamType {
    let function_kind = get_function_kind(sig);

    if let syn::ReturnType::Type(_, ty) = &sig.output {
        if let Some(inner_type) = extract_inner_type_if_future(ty) {
            if is_multimodal_type(inner_type) {
                return OutputParamType {
                    param_type: ParamType::Multimodal,
                    function_kind,
                };
            }
        } else if is_multimodal_type(ty) {
            return OutputParamType {
                param_type: ParamType::Multimodal,
                function_kind,
            };
        }
    }

    OutputParamType {
        param_type: ParamType::Tuple,
        function_kind,
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

fn is_multimodal_type(ty: &Type) -> bool {
    if let Type::Path(type_path) = ty {
        if let Some(seg) = type_path.path.segments.last() {
            return seg.ident == "Multimodal";
        }
    }
    false
}
