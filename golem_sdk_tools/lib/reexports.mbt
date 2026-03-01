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
      has_error: None,
      is_async: None,
      decl_params: Some(to_list(params)),
      params_loc: dummy_loc,
      quantifiers: to_list([]),
      return_type: Some(make_type(fn_.return_type)),
      error_type: @syntax.ErrorType::NoErrorType,
      vis: @syntax.Visibility::Pub(attr=None, loc=dummy_loc),
      attrs: to_list([]),
      doc: @syntax.DocString::empty(),
    }
    impls.push(
      @syntax.Impl::TopFuncDef(
        fun_decl~,
        decl_body=@syntax.DeclBody::DeclBody(
          local_types=to_list([]),
          expr=call_expr,
        ),
        loc=dummy_loc,
      ),
    )
  }
  to_list(impls)
}
