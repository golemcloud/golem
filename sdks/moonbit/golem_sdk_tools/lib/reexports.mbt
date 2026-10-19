///|
/// Generates re-export wrapper functions as AST nodes. Each function delegates
/// to the corresponding function in the SDK's `gen` package.
pub fn generate_reexports(
  fns : Array[FnSignature],
  gen_pkg : String,
) -> @list.List[@syntax.Impl] {
  let impls : Array[@syntax.Impl] = []
  for fn_ in fns {
    let params : Array[@syntax.Parameter] = []
    let args : Array[@syntax.Argument] = []
    for j, param_type in fn_.params {
      let name = "p\{j}"
      params.push(make_param(name, param_type))
      args.push(make_positional_arg(make_ident_expr(name)))
    }
    let call_expr = make_apply(
      make_qualified_expr(gen_pkg, fn_.gen_name),
      args.map(fn(a) { a.value }),
    )
    let fun_decl : @syntax.FunDecl = {
      type_name: None,
      name: make_binder(fn_.user_name),
      is_async: None,
      decl_params: Some(to_list(params)),
      params_loc: dummy_loc,
      quantifiers: to_list([]),
      return_type: Some(make_type(fn_.return_type)),
      error_type: @syntax.ErrorType::NoErrorType,
      vis: @syntax.Visibility::Pub(attr=None, loc=dummy_loc),
      attrs: to_list([]),
      doc: @syntax.DocString::empty(),
      loc: dummy_loc,
    }
    impls.push(
      @syntax.Impl::TopFuncDef(
        fun_decl~,
        decl_body=@syntax.DeclBody::DeclBody(expr=call_expr),
        where_clause=None,
        loc=dummy_loc,
      ),
    )
  }
  to_list(impls)
}

///|
/// Select canonical entry points before referencing typed bindings, so absent
/// interfaces retain neither their lifting nor their post-return cleanup code.
pub fn generate_component_reexports(
  fns : Array[FnSignature],
  has_agents : Bool,
  has_tools : Bool,
  has_middlewares : Bool,
) -> @list.List[@syntax.Impl] {
  let impls : Array[@syntax.Impl] = []
  for fn_ in fns {
    let replacement = match fn_.gen_name {
      "wasmExportDiscoverAgentTypes" if !has_agents => Some("discover")
      "wasmExportDiscoverAgentTypesPostReturn" if !has_agents =>
        Some("post_discover")
      "wasmExportGetDefinition" if !has_agents => Some("definition")
      "wasmExportGetDefinitionPostReturn" if !has_agents =>
        Some("post_definition")
      "wasmExportInitialize" | "wasmExportInvokeGolemAgentGuest" if !has_agents =>
        Some("invoke")
      "wasmExportAsyncInitialize"
      | "wasmExportAsyncInvokeGolemAgentGuest"
      | "wasmExportAsyncSave"
      | "wasmExportAsyncLoad" if !has_agents => Some("callback")
      "wasmExportSave" if !has_agents => Some("save")
      "wasmExportLoad" if !has_agents => Some("load")
      "wasmExportDiscoverTools" if !has_tools => Some("discover")
      "wasmExportDiscoverToolsPostReturn" if !has_tools => Some("post_discover")
      "wasmExportGetTool" if !has_tools => Some("get")
      "wasmExportGetToolPostReturn" if !has_tools => Some("post_get")
      "wasmExportInvokeGolemToolGuest" if !has_tools => Some("invoke")
      "wasmExportAsyncInvokeGolemToolGuest" if !has_tools => Some("callback")
      "wasmExportDiscoverToolMiddlewares" if !has_middlewares =>
        Some("discover")
      "wasmExportDiscoverToolMiddlewaresPostReturn" if !has_middlewares =>
        Some("post_discover")
      "wasmExportGetToolMiddleware" if !has_middlewares => Some("get")
      "wasmExportGetToolMiddlewarePostReturn" if !has_middlewares =>
        Some("post_get")
      "wasmExportInvokeToolMiddleware" if !has_middlewares => Some("invoke")
      "wasmExportAsyncInvokeToolMiddleware" if !has_middlewares =>
        Some("callback")
      _ => None
    }
    let wrappers = match replacement {
      Some(gen_name) =>
        generate_reexports([{ ..fn_, gen_name, }], "minimalExports")
      None => generate_reexports([fn_], "gen")
    }
    wrappers.each(fn(impl_) { impls.push(impl_) })
  }
  to_list(impls)
}
