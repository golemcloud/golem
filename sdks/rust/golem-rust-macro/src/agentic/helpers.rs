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

use convert_case::{Case, Casing};

pub enum InputParamType {
    Tuple,
    Multimodal,
}

pub enum OutputParamType {
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
                        return InputParamType::Multimodal;
                    }
                }
            }
        }
    }
    InputParamType::Tuple
}

pub fn get_output_param_type(sig: &syn::Signature) -> OutputParamType {
    if let syn::ReturnType::Type(_, ty) = &sig.output {
        if let syn::Type::Path(type_path) = &**ty {
            if let Some(seg) = type_path.path.segments.last() {
                if seg.ident == "Multimodal" {
                    // Depends on how exactly multimodal is represented
                    return OutputParamType::Multimodal;
                }
            }
        }
    }
    OutputParamType::Tuple
}

pub fn convert_to_kebab(s: &str) -> String {
    s.to_case(Case::Kebab)
}
