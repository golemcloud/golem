///|
let dummy_pos : @basic.Position = { fname: "", lnum: 0, bol: 0, cnum: 0 }

///|
let dummy_loc : @basic.Location = { start: dummy_pos, end: dummy_pos }

///|
fn make_type(name : String) -> @syntax.Type {
  @syntax.Type::Name(
    constr_id=@syntax.ConstrId::{
      id: @syntax.LongIdent::Ident(name~),
      loc: dummy_loc,
    },
    tys=@list.List::from_array([]),
    loc=dummy_loc,
  )
}

///|
fn make_ident_expr(name : String) -> @syntax.Expr {
  @syntax.Expr::Ident(
    id=@syntax.Var::{ name: @syntax.LongIdent::Ident(name~), loc: dummy_loc },
    loc=dummy_loc,
  )
}

///|
fn make_qualified_expr(pkg : String, id : String) -> @syntax.Expr {
  @syntax.Expr::Ident(
    id=@syntax.Var::{ name: @syntax.LongIdent::Dot(pkg~, id~), loc: dummy_loc },
    loc=dummy_loc,
  )
}

///|
fn make_param(name : String, type_name : String) -> @syntax.Parameter {
  @syntax.Parameter::Positional(
    binder=@syntax.Binder::{ name, loc: dummy_loc },
    ty=Some(make_type(type_name)),
  )
}

///|
fn make_positional_arg(expr : @syntax.Expr) -> @syntax.Argument {
  @syntax.Argument::{ value: expr, kind: @syntax.ArgumentKind::Positional }
}

///|
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
    let call_expr = @syntax.Expr::Apply(
      func=make_qualified_expr(gen_pkg, fn_.gen_name),
      args=@list.List::from_array(args),
      attr=@syntax.ApplyAttr::NoAttr,
      loc=dummy_loc,
    )
    let fun_decl : @syntax.FunDecl = {
      type_name: None,
      name: @syntax.Binder::{ name: fn_.user_name, loc: dummy_loc },
      has_error: None,
      is_async: None,
      decl_params: Some(@list.List::from_array(params)),
      params_loc: dummy_loc,
      quantifiers: @list.List::from_array([]),
      return_type: Some(make_type(fn_.return_type)),
      error_type: @syntax.ErrorType::NoErrorType,
      vis: @syntax.Visibility::Pub(attr=None, loc=dummy_loc),
      attrs: @list.List::from_array([]),
      doc: @syntax.DocString::empty(),
    }
    impls.push(
      @syntax.Impl::TopFuncDef(
        fun_decl~,
        decl_body=@syntax.DeclBody::DeclBody(
          local_types=@list.List::from_array([]),
          expr=call_expr,
        ),
        loc=dummy_loc,
      ),
    )
  }
  @list.List::from_array(impls)
}
