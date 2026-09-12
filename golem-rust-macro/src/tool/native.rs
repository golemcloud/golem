use crate::tool::definition::{build_tool_definition_ir, parse_version, strip_helper_attrs};
use crate::tool::helpers::{
    fresh_internal_ident, normalize_sdk_paths_in_item_trait, resolve_generated_sdk_paths,
};
use crate::tool::ir::ToolDefinitionIr;
use proc_macro::TokenStream;
use proc_macro2::Span;
use quote::{ToTokens, format_ident, quote};
use std::collections::HashSet;
use syn::{FnArg, ItemImpl, ItemTrait, PatType, TraitItem, Type};

pub fn native_tool_definition_impl(
    attrs: TokenStream,
    item: TokenStream,
    native: &syn::Ident,
) -> TokenStream {
    let mut item_trait = syn::parse_macro_input!(item as ItemTrait);
    let version = match parse_version(attrs.into()) {
        Ok(version) => version,
        Err(error) => return error.to_compile_error().into(),
    };
    let canonical = syn::Ident::new("golem_native_tool", Span::call_site());
    let preserved = fresh_internal_ident(
        &item_trait.to_token_stream(),
        "__golem_native_preserved",
        item_trait.ident.span(),
    );
    if native != "crate" {
        normalize_sdk_paths_in_item_trait(&mut item_trait, native, &canonical, &preserved);
    }
    let infrastructure_methods = infrastructure_methods(&item_trait);
    let mut metadata_trait = item_trait.clone();
    let context_ty = match remove_context_parameters(&mut metadata_trait) {
        Ok(context_ty) => context_ty,
        Err(error) => return error.to_compile_error().into(),
    };
    remove_host_result_wrappers(&mut metadata_trait);
    let ir = match build_tool_definition_ir(&metadata_trait, version) {
        Ok(ir) => ir,
        Err(error) => return error.to_compile_error().into(),
    };
    let descriptor_canonical = syn::Ident::new("golem_rust", Span::call_site());
    let resolve = |tokens| {
        let tokens = resolve_generated_sdk_paths(tokens, native, &descriptor_canonical, &preserved);
        resolve_generated_sdk_paths(tokens, native, &canonical, &preserved)
    };
    let descriptor = match crate::tool::descriptor::synthesize_descriptor_fn(&ir) {
        Ok(tokens) => resolve(tokens),
        Err(error) => return error.to_compile_error().into(),
    };
    let descriptor_ident = crate::tool::descriptor::descriptor_fn_ident(&ir.trait_ident);
    strip_helper_attrs(&mut item_trait);
    require_send_futures(&mut item_trait);
    let invoke = match syn::parse2::<TraitItem>(resolve(synthesize_native_invoke(
        &ir,
        &context_ty,
        &infrastructure_methods,
    ))) {
        Ok(invoke) => invoke,
        Err(error) => return error.to_compile_error().into(),
    };
    let descriptor_method: TraitItem = syn::parse_quote! {
        #[doc(hidden)]
        fn __native_tool_descriptor() -> golem_native_tool::Tool {
            #descriptor_ident(&mut golem_native_tool::agentic::ToolBuildCtx::new())
                .and_then(|descriptor| descriptor.try_to_native_tool())
                .expect("native tool descriptor build failed")
        }
    };
    item_trait.items.push(descriptor_method);
    item_trait.items.push(invoke);
    item_trait.items.push(syn::parse_quote! {
        #[doc(hidden)]
        fn native_tool_implementation_annotation() where Self: Sized;
    });
    resolve(quote! {
        #[allow(async_fn_in_trait)]
        #item_trait
        #descriptor
    })
    .into()
}

fn host_result_inner(ty: &Type) -> Option<Type> {
    let Type::Path(path) = ty else { return None };
    let segment = path.path.segments.last()?;
    if segment.ident != "HostResult" {
        return None;
    }
    let syn::PathArguments::AngleBracketed(arguments) = &segment.arguments else {
        return None;
    };
    arguments.args.iter().find_map(|argument| match argument {
        syn::GenericArgument::Type(ty) => Some(ty.clone()),
        _ => None,
    })
}

fn infrastructure_methods(item: &ItemTrait) -> HashSet<String> {
    item.items
        .iter()
        .filter_map(|item| {
            let TraitItem::Fn(method) = item else {
                return None;
            };
            let syn::ReturnType::Type(_, ty) = &method.sig.output else {
                return None;
            };
            host_result_inner(ty).map(|_| method.sig.ident.to_string())
        })
        .collect()
}

fn remove_host_result_wrappers(item: &mut ItemTrait) {
    for item in &mut item.items {
        let TraitItem::Fn(method) = item else {
            continue;
        };
        let syn::ReturnType::Type(_, ty) = &mut method.sig.output else {
            continue;
        };
        if let Some(inner) = host_result_inner(ty) {
            **ty = inner;
        }
    }
}

fn require_send_futures(item: &mut ItemTrait) {
    for trait_item in &mut item.items {
        let TraitItem::Fn(method) = trait_item else {
            continue;
        };
        if method.sig.asyncness.take().is_some() {
            let output = match &method.sig.output {
                syn::ReturnType::Default => syn::parse_quote! { () },
                syn::ReturnType::Type(_, output) => output.as_ref().clone(),
            };
            method.sig.output = syn::parse_quote! {
                -> impl ::std::future::Future<Output = #output> + Send
            };
        }
    }
}

fn remove_context_parameters(item: &mut ItemTrait) -> syn::Result<Type> {
    let mut context_ty = None;
    for trait_item in &mut item.items {
        let TraitItem::Fn(method) = trait_item else {
            continue;
        };
        let Some(FnArg::Typed(first)) = method.sig.inputs.iter().nth(1) else {
            return Err(syn::Error::new_spanned(
                &method.sig,
                "native tool handlers must take `&self` followed by `&mut NativeToolContext`",
            ));
        };
        if !is_context(first) {
            return Err(syn::Error::new_spanned(
                first,
                "the first native tool handler parameter after `&self` must be `&mut NativeToolContext`",
            ));
        }
        let Type::Reference(reference) = first.ty.as_ref() else {
            unreachable!()
        };
        let ty = reference.elem.as_ref().clone();
        if let Some(existing) = &context_ty {
            if existing != &ty {
                return Err(syn::Error::new_spanned(
                    first,
                    "all native tool handlers must use the same context type",
                ));
            }
        } else {
            context_ty = Some(ty);
        }
        method.sig.inputs = method
            .sig
            .inputs
            .clone()
            .into_iter()
            .enumerate()
            .filter_map(|(index, input)| (index != 1).then_some(input))
            .collect();
    }
    context_ty.ok_or_else(|| {
        syn::Error::new_spanned(item, "a native tool must declare at least one handler")
    })
}

fn is_context(arg: &PatType) -> bool {
    let Type::Reference(reference) = arg.ty.as_ref() else {
        return false;
    };
    reference.mutability.is_some()
}

fn terminal_type_is(ty: &Type, expected: &str) -> bool {
    matches!(ty, Type::Path(path) if path.path.segments.last().is_some_and(|segment| segment.ident == expected))
}

fn optional_type_is(ty: &Type, expected: &str) -> bool {
    let Type::Path(path) = ty else { return false };
    let Some(segment) = path.path.segments.last() else {
        return false;
    };
    let syn::PathArguments::AngleBracketed(arguments) = &segment.arguments else {
        return false;
    };
    segment.ident == "Option" && arguments.args.iter().any(|argument| matches!(argument, syn::GenericArgument::Type(inner) if terminal_type_is(inner, expected)))
}

fn is_principal(ty: &Type) -> bool {
    let Type::Path(path) = ty else { return false };
    let segments = path
        .path
        .segments
        .iter()
        .map(|segment| segment.ident.to_string())
        .collect::<Vec<_>>();
    matches!(segments.as_slice(), [sdk, principal] if (sdk == "golem_native_tool" || sdk == "crate") && principal == "Principal")
}
fn is_stdin(ty: &Type) -> bool {
    terminal_type_is(ty, "NativeToolStdin")
}
fn is_optional_stdin(ty: &Type) -> bool {
    optional_type_is(ty, "NativeToolStdin")
}
fn is_stdout(ty: &Type) -> bool {
    terminal_type_is(ty, "NativeToolStdout")
}
fn is_optional_stdout(ty: &Type) -> bool {
    optional_type_is(ty, "NativeToolStdout")
}

fn synthesize_native_invoke(
    ir: &ToolDefinitionIr,
    context_ty: &Type,
    infrastructure_methods: &HashSet<String>,
) -> proc_macro2::TokenStream {
    let tool_name = crate::tool::helpers::to_kebab_case(&ir.trait_ident.to_string());
    let arms = ir.commands.iter().map(|command| {
        let method = &command.method_ident;
        let infrastructure_result = infrastructure_methods.contains(&method.to_string());
        let name = crate::tool::helpers::to_kebab_case(&method.to_string());
        let command_name = command.name_override.clone().unwrap_or(name.clone());
        let aliases = &command.aliases;
        let path_match = if name == tool_name {
            quote! { __invocation.command_path.is_empty() }
        } else {
            quote! {
                __invocation.command_path.as_slice() == [#command_name]
                    #( || __invocation.command_path.as_slice() == [#aliases] )*
            }
        };
        let decode = command.params.iter().map(|param| {
            let ident = &param.ident;
            let ty = &param.ty;
            let field = super::definition::canonical_param_name(ir, command, param, &tool_name);
            if is_principal(ty) {
                quote! { let #ident = __invocation.principal.clone(); }
            } else if is_stdin(ty) {
                quote! { let #ident = match __invocation.stdin.take() { Some(value) => value, None => return Ok(Err(golem_native_tool::NativeToolRpcError::InvalidInput("tool invocation did not contain declared stdin stream".to_string()))) }; }
            } else if is_optional_stdin(ty) {
                quote! { let #ident = __invocation.stdin.take(); }
            } else if is_stdout(ty) {
                quote! { let #ident = match __invocation.stdout.take() { Some(value) => value, None => return Ok(Err(golem_native_tool::NativeToolRpcError::InvalidInput("tool invocation did not contain declared stdout stream".to_string()))) }; }
            } else if is_optional_stdout(ty) {
                quote! { let #ident = __invocation.stdout.take(); }
            } else { quote! {
                let #ident = {
                    let __index = match __fields.iter().position(|value| value.name == #field) { Some(index) => index, None => return Ok(Err(golem_native_tool::NativeToolRpcError::InvalidInput(format!("missing canonical tool input field `{}`", #field)))) };
                    let __field = __fields.remove(__index);
                    match <#ty as golem_native_tool::FromSchema>::from_value(&__field.value) { Ok(value) => value, Err(error) => return Ok(Err(golem_native_tool::NativeToolRpcError::InvalidInput(error.to_string()))) }
                };
            } }
        });
        let args = command.params.iter().map(|param| &param.ident);
        let raw_call = if command.is_async { quote! { self.#method(__context, #(#args),*).await } } else { quote! { self.#method(__context, #(#args),*) } };
        let call = if infrastructure_result { quote! { (#raw_call)? } } else { raw_call };
        if let Some(subtree) = &command.subtree {
            let child_trait = &subtree.path;
            let child_ty = match &command.output {
                syn::ReturnType::Type(_, ty) => ty.as_ref(),
                syn::ReturnType::Default => unreachable!(),
            };
            return quote! {
                if #path_match || __invocation.command_path.first().is_some_and(|__part| __part == #command_name #( || __part == #aliases )*) {
                    #(#decode)*
                    let __child: #child_ty = #call;
                    if !__invocation.command_path.is_empty() {
                        __invocation.command_path.remove(0);
                    }
                    let __child_tool = <#child_ty as #child_trait>::__native_tool_descriptor();
                    let __child_index = match __child_tool.command_index_by_path(&__invocation.command_path) {
                        Some(index) => index,
                        None => return Ok(Err(golem_native_tool::NativeToolRpcError::InvalidCommandPath(__invocation.command_path.clone()))),
                    };
                    let __child_model = match __child_tool.canonical_input_model(__child_index) {
                        Ok(model) => model,
                        Err(error) => return Ok(Err(golem_native_tool::NativeToolRpcError::InvalidInput(error.to_string()))),
                    };
                    let __child_values = match __child_model.fields.iter().map(|field| {
                            __all_fields.iter()
                                .find(|value| value.name == field.name || value.aliases.iter().any(|alias| alias == &field.name))
                                .map(|value| value.value.clone())
                                .ok_or_else(|| golem_native_tool::NativeToolRpcError::InvalidInput(format!("missing canonical tool input field `{}`", field.name)))
                        }).collect::<::std::result::Result<::std::vec::Vec<_>, _>>() {
                        Ok(values) => values,
                        Err(error) => return Ok(Err(error)),
                    };
                    __invocation.input = golem_native_tool::TypedSchemaValue::new(
                        __child_model.record_schema,
                        golem_native_tool::SchemaValue::Record { fields: __child_values },
                    );
                    return <#child_ty as #child_trait>::__native_tool_invoke(&__child, __context, __invocation).await;
                }
            };
        }
        let (ok, err) = super::definition::split_result(&command.output);
        let encode = if err.is_some() {
            if ok.is_some() {
                quote! { match #call { Ok(value) => golem_native_tool::encode_result(&value).map(|value| golem_native_tool::NativeToolStructuredResult { result: Some(value) }), Err(error) => match golem_native_tool::agentic::ToolErrorSchema::to_error_payload_value(&error) { Ok(value) => Err(golem_native_tool::NativeToolRpcError::Custom(value)), Err(error) => Err(golem_native_tool::NativeToolRpcError::InvalidResult(error)) } } }
            } else {
                quote! { match #call { Ok(()) => Ok(golem_native_tool::NativeToolStructuredResult { result: None }), Err(error) => match golem_native_tool::agentic::ToolErrorSchema::to_error_payload_value(&error) { Ok(value) => Err(golem_native_tool::NativeToolRpcError::Custom(value)), Err(error) => Err(golem_native_tool::NativeToolRpcError::InvalidResult(error)) } } }
            }
        } else if ok.is_some() {
            quote! { golem_native_tool::encode_result(&#call).map(|value| golem_native_tool::NativeToolStructuredResult { result: Some(value) }) }
        } else {
            quote! { #call; Ok(golem_native_tool::NativeToolStructuredResult { result: None }) }
        };
        quote! {
            if #path_match {
                #(#decode)*
                let __result: golem_native_tool::NativeToolRpcResult = { #encode };
                return Ok(__result);
            }
        }
    });
    quote! {
        #[doc(hidden)]
        fn __native_tool_invoke<'a>(
            &'a self,
            __context: &'a mut #context_ty,
            mut __invocation: golem_native_tool::NativeToolInvocation,
        ) -> golem_native_tool::NativeToolFuture<'a, golem_native_tool::HostError>
        where Self: Sync + 'a
        {
            ::std::boxed::Box::pin(async move {
                let __tool = Self::__native_tool_descriptor();
                let __command_index = match __tool.command_index_by_path(&__invocation.command_path) {
                    Some(index) => index,
                    None => return Ok(Err(golem_native_tool::NativeToolRpcError::InvalidCommandPath(__invocation.command_path.clone()))),
                };
                let (_, __input_value) = __invocation.input.clone().into_parts();
                let mut __fields = __tool.decode_canonical_input_record(__command_index, __input_value)
                    .map_err(|error| golem_native_tool::NativeToolRpcError::InvalidInput(error.to_string()));
                let mut __fields = match __fields { Ok(fields) => fields, Err(error) => return Ok(Err(error)) };
                let __all_fields = __fields.clone();
                #(#arms)*
                Ok(Err(golem_native_tool::NativeToolRpcError::InvalidCommandPath(__invocation.command_path)))
            })
        }
    }
}

pub fn native_tool_implementation_impl(
    _attrs: TokenStream,
    item: TokenStream,
    native: &syn::Ident,
) -> TokenStream {
    let mut item_impl = syn::parse_macro_input!(item as ItemImpl);
    let Some((_, trait_path, _)) = &item_impl.trait_ else {
        return syn::Error::new_spanned(
            &item_impl.self_ty,
            "#[tool_implementation] must be applied to a trait implementation",
        )
        .to_compile_error()
        .into();
    };
    let self_ty = item_impl.self_ty.clone();
    let context_ty = item_impl.items.iter().find_map(|item| {
        let syn::ImplItem::Fn(method) = item else {
            return None;
        };
        let FnArg::Typed(first) = method.sig.inputs.iter().nth(1)? else {
            return None;
        };
        let Type::Reference(reference) = first.ty.as_ref() else {
            return None;
        };
        Some(reference.elem.as_ref().clone())
    });
    let Some(context_ty) = context_ty else {
        return syn::Error::new_spanned(
            &item_impl.self_ty,
            "native tool implementation has no handler context type",
        )
        .to_compile_error()
        .into();
    };
    item_impl.items.push(
        syn::parse_quote! { fn native_tool_implementation_annotation() where Self: Sized {} },
    );
    let identity = format!(
        "{}_{}",
        self_ty.to_token_stream(),
        trait_path.to_token_stream()
    );
    let identity = identity
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .collect::<String>();
    let wrapper = format_ident!("__GolemNativeToolInvoker{identity}");
    let canonical = syn::Ident::new("golem_native_tool", Span::call_site());
    let generated = quote! {
        #item_impl
        pub struct #wrapper(pub #self_ty);
        impl #wrapper {
            pub fn new(implementation: #self_ty) -> Self { Self(implementation) }
        }
        impl golem_native_tool::NativeToolInvoker<#context_ty, golem_native_tool::HostError> for #wrapper {
            fn metadata(&self) -> golem_native_tool::Tool { <#self_ty as #trait_path>::__native_tool_descriptor() }
            fn invoke<'a>(&'a self, context: &'a mut #context_ty, invocation: golem_native_tool::NativeToolInvocation) -> golem_native_tool::NativeToolFuture<'a, golem_native_tool::HostError> {
                <#self_ty as #trait_path>::__native_tool_invoke(&self.0, context, invocation)
            }
        }
        impl #self_ty {
            pub fn native_tool_invoker(self) -> impl golem_native_tool::NativeToolInvoker<#context_ty, golem_native_tool::HostError> {
                #wrapper::new(self)
            }
        }
    };
    resolve_generated_sdk_paths(
        generated,
        native,
        &canonical,
        &format_ident!("__unused_native_marker"),
    )
    .into()
}
