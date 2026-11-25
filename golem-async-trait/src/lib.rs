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

//! Post-expansion future construction for selected, locally owned trait implementors.
//!
//! Place [`region_simple`] after `async_trait` on an implementation. Public
//! signatures stay unchanged; each wrapper returns one boxed future built by a
//! private inherent method. Default trait bodies are not transformed.
//!
//! The self type must admit an inherent implementation, and all implementation
//! type/const parameters must be constrained by it. Source methods must not
//! declare lifetimes. After expansion, explicit `lifeN` lifetimes cannot be
//! distinguished from async-trait's generated lifetimes; check this at the source.
//! The implementation lifetime `__region_capture` is reserved.

use proc_macro::TokenStream;
use proc_macro2::Span;
use quote::{ToTokens, format_ident, quote};
use std::collections::BTreeSet;
use std::hash::{DefaultHasher, Hash, Hasher};
use syn::spanned::Spanned;
use syn::visit_mut::{self, VisitMut};
use syn::{
    FnArg, GenericParam, ImplItem, ItemImpl, Lifetime, Pat, TypePath, WherePredicate, parse_quote,
};

struct CaptureLifetime {
    generated: BTreeSet<String>,
}

impl VisitMut for CaptureLifetime {
    fn visit_lifetime_mut(&mut self, value: &mut Lifetime) {
        if self.generated.contains(&value.ident.to_string()) {
            *value = Lifetime::new("'__region_capture", value.apostrophe);
        }
    }
}

struct AssociatedTypes {
    trait_path: syn::Path,
    names: BTreeSet<String>,
}

impl VisitMut for AssociatedTypes {
    fn visit_type_path_mut(&mut self, path: &mut TypePath) {
        if path.qself.is_none()
            && path.path.segments.len() == 2
            && path.path.segments[0].ident == "Self"
            && self
                .names
                .contains(&path.path.segments[1].ident.to_string())
        {
            let trait_path = &self.trait_path;
            let associated = &path.path.segments[1];
            *path = parse_quote!(<Self as #trait_path>::#associated);
        } else {
            visit_mut::visit_type_path_mut(self, path);
        }
    }
}

fn reject_conditional_instrumentation(meta: &syn::Meta) -> syn::Result<()> {
    if let syn::Meta::List(list) = meta
        && list.path.is_ident("cfg_attr")
    {
        let args = list.parse_args_with(
            syn::punctuated::Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated,
        )?;
        for attr in args.iter().skip(1) {
            if attr.path().segments.last().unwrap().ident == "instrument" {
                return Err(syn::Error::new_spanned(
                    attr,
                    "region_simple does not support instrument inside cfg_attr; use a direct instrument attribute",
                ));
            }
            reject_conditional_instrumentation(attr)?;
        }
    }
    Ok(())
}

fn transform(mut item: ItemImpl) -> syn::Result<proc_macro2::TokenStream> {
    let Some((_, trait_path, _)) = &item.trait_ else {
        return Err(syn::Error::new_spanned(
            &item,
            "region_simple requires an expanded trait implementation",
        ));
    };
    if let Some(lifetime) = item
        .generics
        .lifetimes()
        .find(|p| p.lifetime.ident == "__region_capture")
    {
        return Err(syn::Error::new_spanned(
            lifetime,
            "the implementation lifetime '__region_capture is reserved by region_simple",
        ));
    }
    let trait_path = trait_path.clone();
    let mut trait_hasher = DefaultHasher::new();
    trait_path
        .to_token_stream()
        .to_string()
        .hash(&mut trait_hasher);
    let trait_key = trait_hasher.finish();
    let trait_name = trait_path
        .segments
        .last()
        .unwrap()
        .ident
        .to_string()
        .to_lowercase();
    let inherited: BTreeSet<_> = item
        .generics
        .type_params()
        .map(|t| t.ident.to_string())
        .collect();
    let mut associated = AssociatedTypes {
        trait_path: trait_path.clone(),
        names: item
            .items
            .iter()
            .filter_map(|m| match m {
                ImplItem::Type(t) => Some(t.ident.to_string()),
                _ => None,
            })
            .collect(),
    };
    let mut helper_impl = item.clone();
    helper_impl.trait_ = None;
    helper_impl.unsafety = None;
    helper_impl.items.clear();
    for member in &mut item.items {
        let ImplItem::Fn(method) = member else {
            continue;
        };
        if method.sig.asyncness.is_some() {
            return Err(syn::Error::new_spanned(
                &method.sig,
                "place region_simple after async_trait",
            ));
        }
        if !method
            .sig
            .generics
            .lifetimes()
            .any(|p| p.lifetime.ident == "async_trait")
        {
            continue;
        }
        let boxed = match method.block.stmts.as_slice() {
            [syn::Stmt::Expr(syn::Expr::Call(call), None)] => match call.func.as_ref() {
                syn::Expr::Path(path) => {
                    path.path
                        .segments
                        .iter()
                        .map(|s| s.ident.to_string())
                        .collect::<Vec<_>>()
                        .ends_with(&["Box".into(), "pin".into()])
                        && call.args.len() == 1
                        && matches!(call.args.first(), Some(syn::Expr::Async(_)))
                }
                _ => false,
            },
            _ => false,
        };
        if !boxed {
            return Err(syn::Error::new_spanned(
                &method.block,
                "region_simple requires async_trait's Box::pin(async move { ... }) expansion",
            ));
        }
        let generated: BTreeSet<_> = method
            .sig
            .generics
            .lifetimes()
            .map(|p| p.lifetime.ident.to_string())
            .collect();
        if generated.iter().any(|n| {
            n != "async_trait"
                && !n
                    .strip_prefix("life")
                    .is_some_and(|s| s.parse::<usize>().is_ok())
        }) {
            return Err(syn::Error::new_spanned(
                &method.sig,
                "explicit method lifetimes are not supported by region_simple",
            ));
        }
        let mut helper = method.clone();
        helper.sig.ident = format_ident!(
            "__region_{}_{:016x}_{}",
            trait_name,
            trait_key,
            method.sig.ident,
            span = method.sig.ident.span()
        );
        helper.sig.generics.params = helper
            .sig
            .generics
            .params
            .into_iter()
            .filter(|p| !matches!(p, GenericParam::Lifetime(_)))
            .collect();
        helper
            .sig
            .generics
            .params
            .insert(0, parse_quote!('__region_capture));
        if let Some(clause) = &mut helper.sig.generics.where_clause {
            clause.predicates = clause.predicates.clone().into_iter().filter_map(|p| match p {
                WherePredicate::Lifetime(p) if generated.contains(&p.lifetime.ident.to_string()) => None,
                WherePredicate::Type(mut p) => {
                    let name = p.bounded_ty.to_token_stream().to_string();
                    if name == "Self" || inherited.contains(&name) {
                        p.bounds = p.bounds.into_iter().filter(|b| !matches!(b,
                            syn::TypeParamBound::Lifetime(l) if generated.contains(&l.ident.to_string()))).collect();
                    }
                    if p.bounds.is_empty() { None } else { Some(WherePredicate::Type(p)) }
                },
                other => Some(other),
            }).collect();
            if clause.predicates.is_empty() {
                helper.sig.generics.where_clause = None;
            }
        }
        CaptureLifetime { generated }.visit_signature_mut(&mut helper.sig);
        associated.visit_signature_mut(&mut helper.sig);
        associated.visit_block_mut(&mut helper.block);
        let mut import_path = trait_path.clone();
        for segment in &mut import_path.segments {
            segment.arguments = syn::PathArguments::None;
        }
        if let syn::Stmt::Expr(syn::Expr::Call(call), _) = &mut helper.block.stmts[0]
            && let Some(syn::Expr::Async(body)) = call.args.first_mut()
        {
            body.block.stmts.insert(
                0,
                parse_quote! {
                    #[allow(unused_imports)]
                    use #import_path as _;
                },
            );
        }
        for attr in &mut helper.attrs {
            reject_conditional_instrumentation(&attr.meta)?;
            if attr.path().segments.last().unwrap().ident == "instrument" {
                let args = match &attr.meta {
                    syn::Meta::List(list) => list.tokens.clone(),
                    _ => proc_macro2::TokenStream::new(),
                };
                let tokens: Vec<_> = args.clone().into_iter().collect();
                let has_name = tokens
                    .split(|token| matches!(token, proc_macro2::TokenTree::Punct(p) if p.as_char() == ','))
                    .any(|arg| match arg {
                        [proc_macro2::TokenTree::Ident(i), proc_macro2::TokenTree::Punct(p), ..]
                            if i == "name" && p.as_char() == '=' => true,
                        [proc_macro2::TokenTree::Literal(literal)] =>
                            syn::parse2::<syn::LitStr>(quote!(#literal)).is_ok(),
                        _ => false,
                    });
                if !has_name {
                    let name = method.sig.ident.to_string();
                    let path = attr.path().clone();
                    *attr = syn::parse_quote_spanned!(attr.span()=> #[#path(name = #name, #args)]);
                }
            }
        }
        method
            .attrs
            .retain(|a| a.path().segments.last().unwrap().ident != "instrument");
        let actuals: syn::Result<Vec<_>> = method
            .sig
            .inputs
            .iter()
            .map(|a| match a {
                FnArg::Receiver(_) => Ok(quote!(self)),
                FnArg::Typed(a) => match a.pat.as_ref() {
                    Pat::Ident(p) => {
                        let name = &p.ident;
                        Ok(quote!(#name))
                    }
                    _ => Err(syn::Error::new_spanned(
                        a,
                        "region_simple requires expanded identifier arguments",
                    )),
                },
            })
            .collect();
        let actuals = actuals?;
        let generics: Vec<_> = method
            .sig
            .generics
            .params
            .iter()
            .filter_map(|p| match p {
                GenericParam::Type(t) => Some(t.ident.to_token_stream()),
                GenericParam::Const(c) => Some(c.ident.to_token_stream()),
                _ => None,
            })
            .collect();
        let helper_name = &helper.sig.ident;
        method.block = if generics.is_empty() {
            parse_quote!({ Self::#helper_name(#(#actuals),*) })
        } else {
            parse_quote!({ Self::#helper_name::<#(#generics),*>(#(#actuals),*) })
        };
        helper_impl.items.push(ImplItem::Fn(helper));
    }
    if helper_impl.items.is_empty() {
        return Err(syn::Error::new(
            Span::call_site(),
            "region_simple found no expanded async_trait methods",
        ));
    }
    Ok(quote!(#item #helper_impl))
}

/// Keep async-trait's boxed signature while simplifying its construction regions.
///
/// Unsupported explicit method lifetimes fail before helper construction:
///
/// ```compile_fail
/// #[async_trait::async_trait]
/// trait Operation { async fn run<'a>(&'a self); }
/// struct Subject;
/// #[async_trait::async_trait]
/// #[golem_async_trait::region_simple]
/// impl Operation for Subject { async fn run<'a>(&'a self) {} }
/// ```
///
/// Attribute order is significant:
///
/// ```compile_fail
/// #[async_trait::async_trait]
/// trait Operation { async fn run(&self); }
/// struct Subject;
/// #[golem_async_trait::region_simple]
/// #[async_trait::async_trait]
/// impl Operation for Subject { async fn run(&self) {} }
/// ```
///
/// Conditional instrumentation is unsupported, including nested conditions:
///
/// ```compile_fail
/// #[async_trait::async_trait]
/// trait Operation { async fn run(&self); }
/// struct Subject;
/// #[async_trait::async_trait]
/// #[golem_async_trait::region_simple]
/// impl Operation for Subject {
///     #[cfg_attr(all(), tracing::instrument(skip_all))]
///     async fn run(&self) {}
/// }
/// ```
///
/// ```compile_fail
/// #[async_trait::async_trait]
/// trait Operation { async fn run(&self); }
/// struct Subject;
/// #[async_trait::async_trait]
/// #[golem_async_trait::region_simple]
/// impl Operation for Subject {
///     #[cfg_attr(all(), cfg_attr(all(), tracing::instrument(skip_all)))]
///     async fn run(&self) {}
/// }
/// ```
#[proc_macro_attribute]
pub fn region_simple(args: TokenStream, input: TokenStream) -> TokenStream {
    if !args.is_empty() {
        return syn::Error::new(Span::call_site(), "region_simple takes no arguments")
            .into_compile_error()
            .into();
    }
    let item = syn::parse_macro_input!(input as ItemImpl);
    transform(item)
        .unwrap_or_else(|e| e.into_compile_error())
        .into()
}
