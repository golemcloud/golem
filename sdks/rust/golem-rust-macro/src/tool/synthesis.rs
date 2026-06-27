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

//! Shared token emission helpers used to synthesize the runtime tool metadata
//! (`ExtendedToolType`, `ExtendedErrorCase`, ...) from the parsed IR.

use crate::tool::ir::{DocIr, ErrorKindIr};
use proc_macro2::TokenStream;
use quote::quote;

/// Emits a `golem_rust::agentic::Doc` value from a [`DocIr`].
pub fn doc_tokens(doc: &DocIr) -> TokenStream {
    let summary = &doc.summary;
    let description = &doc.description;
    let examples = doc.examples.iter().map(|ex| {
        let title = &ex.title;
        let body = &ex.body;
        quote! {
            golem_rust::agentic::Example {
                title: #title.to_string(),
                body: #body.to_string(),
            }
        }
    });
    quote! {
        golem_rust::agentic::Doc {
            summary: #summary.to_string(),
            description: #description.to_string(),
            examples: vec![ #(#examples),* ],
        }
    }
}

/// Emits a `wire::ErrorKind` value.
pub fn error_kind_tokens(kind: ErrorKindIr) -> TokenStream {
    match kind {
        ErrorKindIr::UsageError => quote! {
            golem_rust::golem_agentic::golem::tool::common::ErrorKind::UsageError
        },
        ErrorKindIr::RuntimeError => quote! {
            golem_rust::golem_agentic::golem::tool::common::ErrorKind::RuntimeError
        },
    }
}
