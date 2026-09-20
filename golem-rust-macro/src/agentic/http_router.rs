// Copyright 2024-2026 Golem Cloud
// Licensed under the Apache License, Version 2.0 (https://www.apache.org/licenses/LICENSE-2.0).

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::{Expr, ImplItem, ItemImpl, Token, parse::Parser, punctuated::Punctuated};

/// Lower a fixed SDK trait implementation through the ordinary agent macros.
pub fn expand(attrs: TokenStream, item: TokenStream) -> syn::Result<TokenStream> {
    let implementation: ItemImpl = syn::parse2(item)?;
    if !implementation.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(
            &implementation.generics,
            "HTTP router registrations must name a concrete implementation, not a generic impl",
        ));
    }
    let Some((None, trait_path, _)) = &implementation.trait_ else {
        return Err(syn::Error::new_spanned(
            &implementation,
            "#[http_router] requires impl HttpRouter for YourType",
        ));
    };
    if trait_path
        .segments
        .last()
        .is_none_or(|segment| segment.ident != "HttpRouter")
    {
        return Err(syn::Error::new_spanned(
            trait_path,
            "#[http_router] requires the SDK HttpRouter trait",
        ));
    }
    let options = Punctuated::<Expr, Token![,]>::parse_terminated.parse2(attrs)?;
    let mut keys = std::collections::HashSet::new();
    for option in &options {
        let Expr::Assign(assign) = option else {
            return Err(syn::Error::new_spanned(
                option,
                "expected name, mount, auth, cors, or static_files = value",
            ));
        };
        let Expr::Path(path) = &*assign.left else {
            return Err(syn::Error::new_spanned(
                option,
                "expected a router option name",
            ));
        };
        let name = path
            .path
            .get_ident()
            .map(ToString::to_string)
            .unwrap_or_default();
        if !matches!(
            name.as_str(),
            "name" | "mount" | "auth" | "cors" | "static_files"
        ) {
            return Err(syn::Error::new_spanned(
                option,
                "unknown router option; use name, mount, auth, cors, or static_files (routers are always ephemeral with snapshots disabled)",
            ));
        }
        if !keys.insert(name) {
            return Err(syn::Error::new_spanned(option, "duplicate router option"));
        }
    }
    for required in ["name", "mount"] {
        if !keys.contains(required) {
            return Err(syn::Error::new_spanned(
                &implementation.self_ty,
                format!("#[http_router] requires {required} = \"...\""),
            ));
        }
    }
    let mut handle = false;
    let mut openapi = false;
    let mut constructor = false;
    for item in &implementation.items {
        match item {
            ImplItem::Type(item) if item.ident == "Config" => {}
            ImplItem::Fn(method) => {
                for attr in &method.attrs {
                    if !attr.path().is_ident("doc") && !attr.path().is_ident("allow") {
                        return Err(syn::Error::new_spanned(
                            attr,
                            "router methods use fixed roles; put HTTP policy on #[http_router], and do not conditionally compile individual roles",
                        ));
                    }
                }
                match method.sig.ident.to_string().as_str() {
                    "new" => constructor = true,
                    "handle" => handle = true,
                    "openapi" => openapi = true,
                    _ => {
                        return Err(syn::Error::new_spanned(
                            &method.sig,
                            "router methods are new, handle, and openapi; put helpers in an inherent impl",
                        ));
                    }
                }
            }
            _ => {
                return Err(syn::Error::new_spanned(
                    item,
                    "expected type Config or a router method",
                ));
            }
        }
    }
    if !constructor {
        return Err(syn::Error::new_spanned(
            &implementation,
            "implement fn new(config: Config<Self::Config>) -> Self",
        ));
    }
    let self_ty = &implementation.self_ty;
    let self_tokens = quote!(#self_ty).to_string();
    let agent_trait = (0..)
        .map(|index| format!("__GolemHttpRouterAgent{index}"))
        .find(|name| {
            !self_tokens.contains(name) && !self_tokens.contains(&format!("__{name}Initiator"))
        })
        .map(|name| format_ident!("{name}"))
        .unwrap();
    let cfg = implementation
        .attrs
        .iter()
        .filter(|attr| attr.path().is_ident("cfg") || attr.path().is_ident("cfg_attr"));
    let handle_decl = handle.then(|| quote! {
        #[endpoint(any = "/")]
        async fn handle(&self, request: golem_rust::agentic::HttpRequest) -> golem_rust::agentic::HttpResponse;
    });
    let handle_impl = handle.then(|| quote! {
        async fn handle(&self, request: golem_rust::agentic::HttpRequest) -> golem_rust::agentic::HttpResponse {
            <Self as golem_rust::agentic::HttpRouter>::handle(self, request).await
        }
    });
    let provider_decl = openapi.then(|| quote! { async fn openapi(&self) -> String; });
    let provider_impl = openapi.then(|| quote! {
        async fn openapi(&self) -> String { <Self as golem_rust::agentic::HttpRouter>::openapi(self).await }
    });
    let provider_option = openapi.then(|| quote! { openapi_provider_method = "openapi", });
    Ok(quote! {
        #implementation
        #(#cfg)*
        const _: () = {
            #[allow(unused_imports)]
            use golem_rust::endpoint;
            #[golem_rust::agent_definition(
                kind = "http-router", ephemeral, snapshotting = "disabled",
                #provider_option #options
            )]
            trait #agent_trait {
                fn new(#[agent_config] config: golem_rust::agentic::Config<<#self_ty as golem_rust::agentic::HttpRouter>::Config>) -> Self;
                #handle_decl
                #provider_decl
            }
            #[golem_rust::agent_implementation]
            impl #agent_trait for #self_ty {
                fn new(#[agent_config] config: golem_rust::agentic::Config<<#self_ty as golem_rust::agentic::HttpRouter>::Config>) -> Self {
                    <Self as golem_rust::agentic::HttpRouter>::new(config)
                }
                #handle_impl
                #provider_impl
            }
        };
    })
}
