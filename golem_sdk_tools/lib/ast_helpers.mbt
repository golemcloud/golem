///|
let dummy_pos : @basic.Position = { fname: "", lnum: 0, bol: 0, cnum: 0 }

///|
let dummy_loc : @basic.Location = { start: dummy_pos, end: dummy_pos }

// --- Primitives ---

///|
fn make_binder(name : String) -> @syntax.Binder {
  @syntax.Binder::{ name, loc: dummy_loc }
}

///|
fn make_label(name : String) -> @syntax.Label {
  @syntax.Label::{ name, loc: dummy_loc }
}

///|
fn[T] to_list(arr : Array[T]) -> @list.List[T] {
  @list.List::from_array(arr)
}

// --- Types ---

///|
fn make_type(name : String) -> @syntax.Type {
  @syntax.Type::Name(
    constr_id=@syntax.ConstrId::{
      id: @syntax.LongIdent::Ident(name~),
      loc: dummy_loc,
    },
    tys=to_list([]),
    loc=dummy_loc,
  )
}

///|
fn make_parameterized_type(
  name : String,
  type_args : Array[@syntax.Type],
) -> @syntax.Type {
  @syntax.Type::Name(
    constr_id=@syntax.ConstrId::{
      id: @syntax.LongIdent::Ident(name~),
      loc: dummy_loc,
    },
    tys=to_list(type_args),
    loc=dummy_loc,
  )
}

///|
fn make_qualified_parameterized_type(
  pkg : String,
  name : String,
  type_args : Array[@syntax.Type],
) -> @syntax.Type {
  @syntax.Type::Name(
    constr_id=@syntax.ConstrId::{
      id: @syntax.LongIdent::Dot(pkg~, id=name),
      loc: dummy_loc,
    },
    tys=to_list(type_args),
    loc=dummy_loc,
  )
}

///|
fn make_option_type(inner : @syntax.Type) -> @syntax.Type {
  @syntax.Type::Option(ty=inner, loc=dummy_loc, question_loc=dummy_loc)
}

// --- Expressions: Literals ---

///|
fn make_string_expr(s : String) -> @syntax.Expr {
  @syntax.Expr::Constant(c=@syntax.Constant::String(s), loc=dummy_loc)
}

///|
fn make_int_expr(n : Int) -> @syntax.Expr {
  @syntax.Expr::Constant(c=@syntax.Constant::Int(n.to_string()), loc=dummy_loc)
}

// --- Expressions: Identifiers ---

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
fn make_method_ref(type_name : String, method_name : String) -> @syntax.Expr {
  @syntax.Expr::Method(
    type_name=@syntax.TypeName::{
      name: @syntax.LongIdent::Ident(name=type_name),
      is_object: false,
      loc: dummy_loc,
    },
    method_name=make_label(method_name),
    loc=dummy_loc,
  )
}

// --- Expressions: Constructors ---

///|
fn make_constr_no_args(name : String) -> @syntax.Expr {
  @syntax.Expr::Constr(
    constr=@syntax.Constructor::{
      name: @syntax.ConstrName::{ name, loc: dummy_loc },
      extra_info: @syntax.ConstructorExtraInfo::NoExtraInfo,
      loc: dummy_loc,
    },
    loc=dummy_loc,
  )
}

///|
fn make_qualified_constr(
  pkg : String,
  type_name : String,
  variant : String,
) -> @syntax.Expr {
  @syntax.Expr::Constr(
    constr=@syntax.Constructor::{
      name: @syntax.ConstrName::{ name: variant, loc: dummy_loc },
      extra_info: @syntax.ConstructorExtraInfo::TypeName(@syntax.TypeName::{
        name: @syntax.LongIdent::Dot(pkg~, id=type_name),
        is_object: false,
        loc: dummy_loc,
      }),
      loc: dummy_loc,
    },
    loc=dummy_loc,
  )
}

// --- Expressions: Application & Operators ---

///|
fn make_positional_arg(expr : @syntax.Expr) -> @syntax.Argument {
  @syntax.Argument::{ value: expr, kind: @syntax.ArgumentKind::Positional }
}

///|
fn make_apply(func : @syntax.Expr, args : Array[@syntax.Expr]) -> @syntax.Expr {
  @syntax.Expr::Apply(
    func~,
    args=to_list(args.map(make_positional_arg)),
    attr=@syntax.ApplyAttr::NoAttr,
    loc=dummy_loc,
  )
}

///|
fn make_dot_apply(
  self_ : @syntax.Expr,
  method_name : String,
  args : Array[@syntax.Expr],
) -> @syntax.Expr {
  @syntax.Expr::DotApply(
    self=self_,
    method_name=make_label(method_name),
    args=to_list(args.map(make_positional_arg)),
    return_self=false,
    attr=@syntax.ApplyAttr::NoAttr,
    loc=dummy_loc,
  )
}

///|
fn make_infix(
  op : String,
  lhs : @syntax.Expr,
  rhs : @syntax.Expr,
) -> @syntax.Expr {
  @syntax.Expr::Infix(
    op=@syntax.Var::{ name: @syntax.LongIdent::Ident(name=op), loc: dummy_loc },
    lhs~,
    rhs~,
    loc=dummy_loc,
  )
}

// --- Expressions: Collections & Composites ---

///|
fn make_record(fields : Array[@syntax.FieldDef]) -> @syntax.Expr {
  @syntax.Expr::Record(
    type_name=None,
    fields=to_list(fields),
    trailing=@syntax.TrailingMark::Comma,
    loc=dummy_loc,
  )
}

///|
fn make_record_with_type(
  pkg : String,
  type_name : String,
  fields : Array[@syntax.FieldDef],
) -> @syntax.Expr {
  @syntax.Expr::Record(
    type_name=Some(@syntax.TypeName::{
      name: @syntax.LongIdent::Dot(pkg~, id=type_name),
      is_object: false,
      loc: dummy_loc,
    }),
    fields=to_list(fields),
    trailing=@syntax.TrailingMark::Comma,
    loc=dummy_loc,
  )
}

///|
fn make_field(name : String, expr : @syntax.Expr) -> @syntax.FieldDef {
  @syntax.FieldDef::{
    label: make_label(name),
    expr,
    is_pun: false,
    loc: dummy_loc,
  }
}

///|
fn make_array(elems : Array[@syntax.Expr]) -> @syntax.Expr {
  @syntax.Expr::Array(exprs=to_list(elems), loc=dummy_loc)
}

///|
fn make_tuple(elems : Array[@syntax.Expr]) -> @syntax.Expr {
  @syntax.Expr::Tuple(exprs=to_list(elems), loc=dummy_loc)
}

///|
fn make_array_get(array : @syntax.Expr, index : @syntax.Expr) -> @syntax.Expr {
  @syntax.Expr::ArrayGet(array~, index~, loc=dummy_loc)
}

///|
fn make_constraint_expr(expr : @syntax.Expr, ty : @syntax.Type) -> @syntax.Expr {
  @syntax.Expr::Constraint(expr~, ty~, loc=dummy_loc)
}

// --- Expressions: Control Flow ---

///|
fn make_let(
  name : String,
  expr : @syntax.Expr,
  body : @syntax.Expr,
) -> @syntax.Expr {
  @syntax.Expr::Let(
    pattern=@syntax.Pattern::Var(make_binder(name)),
    expr~,
    body~,
    loc=dummy_loc,
  )
}

///|
fn make_let_typed(
  name : String,
  ty : @syntax.Type,
  expr : @syntax.Expr,
  body : @syntax.Expr,
) -> @syntax.Expr {
  @syntax.Expr::Let(
    pattern=@syntax.Pattern::Constraint(
      pat=@syntax.Pattern::Var(make_binder(name)),
      ty~,
      loc=dummy_loc,
    ),
    expr~,
    body~,
    loc=dummy_loc,
  )
}

///|
fn make_raise(err_value : @syntax.Expr) -> @syntax.Expr {
  @syntax.Expr::Raise(err_value~, loc=dummy_loc)
}

///|
fn make_guard_else(
  cond : @syntax.Expr,
  otherwise : @syntax.Expr,
  body : @syntax.Expr,
) -> @syntax.Expr {
  @syntax.Expr::Guard(cond~, otherwise=Some(otherwise), body~, loc=dummy_loc)
}

///|
fn make_try_catch(
  body : @syntax.Expr,
  catch_cases : Array[@syntax.Case],
) -> @syntax.Expr {
  @syntax.Expr::Try(
    body~,
    catch_=to_list(catch_cases),
    catch_all=false,
    try_else=None,
    has_try=true,
    try_loc=dummy_loc,
    catch_loc=dummy_loc,
    else_loc=dummy_loc,
    loc=dummy_loc,
  )
}

///|
fn make_case(pattern : @syntax.Pattern, body : @syntax.Expr) -> @syntax.Case {
  @syntax.Case::{ pattern, body, guard_: None }
}

///|
fn make_lambda(
  param_names : Array[String],
  body : @syntax.Expr,
) -> @syntax.Expr {
  let parameters = param_names.map(fn(name) {
    @syntax.Parameter::Positional(binder=make_binder(name), ty=None)
  })
  @syntax.Expr::Function(
    func=@syntax.Func::{
      body,
      error_type: @syntax.ErrorType::NoErrorType,
      has_error: None,
      is_async: None,
      kind: @syntax.FnKind::Lambda,
      loc: dummy_loc,
      parameters: to_list(parameters),
      params_loc: dummy_loc,
      return_type: None,
    },
    loc=dummy_loc,
  )
}

// --- Expressions: Sequences ---

///|
fn make_sequence(exprs : Array[@syntax.Expr]) -> @syntax.Expr {
  if exprs.length() == 1 {
    exprs[0]
  } else {
    let n = exprs.length()
    let init : Array[@syntax.Expr] = []
    for i in 0..<(n - 1) {
      init.push(exprs[i])
    }
    @syntax.Expr::Sequence(
      exprs=to_list(init),
      last_expr=exprs[n - 1],
      loc=dummy_loc,
    )
  }
}

// --- Top-level Impl nodes ---

///|
fn make_init_block(body : @syntax.Expr) -> @syntax.Impl {
  @syntax.Impl::TopExpr(
    expr=body,
    is_main=false,
    local_types=to_list([]),
    is_async=None,
    loc=dummy_loc,
  )
}

///|
fn make_param(name : String, type_name : String) -> @syntax.Parameter {
  @syntax.Parameter::Positional(
    binder=make_binder(name),
    ty=Some(make_type(type_name)),
  )
}

///|
fn make_var_pattern(name : String) -> @syntax.Pattern {
  @syntax.Pattern::Var(make_binder(name))
}

///|
fn make_match(expr : @syntax.Expr, cases : Array[@syntax.Case]) -> @syntax.Expr {
  @syntax.Expr::Match(
    expr~,
    cases=to_list(cases),
    match_loc=dummy_loc,
    loc=dummy_loc,
  )
}

///|
fn make_string_pattern(s : String) -> @syntax.Pattern {
  @syntax.Pattern::Constant(c=@syntax.Constant::String(s), loc=dummy_loc)
}

///|
fn make_top_impl(
  self_type_name : String,
  trait_pkg : String,
  trait_name : String,
  method_name : String,
  params : Array[String],
  body : @syntax.Expr,
) -> @syntax.Impl {
  @syntax.Impl::TopImpl(
    self_ty=Some(
      @syntax.Type::Name(
        constr_id=@syntax.ConstrId::{
          id: @syntax.LongIdent::Ident(name=self_type_name),
          loc: dummy_loc,
        },
        tys=to_list([]),
        loc=dummy_loc,
      ),
    ),
    trait_=@syntax.TypeName::{
      name: @syntax.LongIdent::Dot(pkg=trait_pkg, id=trait_name),
      is_object: false,
      loc: dummy_loc,
    },
    method_name=make_binder(method_name),
    has_error=None,
    quantifiers=to_list([]),
    params=to_list(
      params.map(fn(name) {
        @syntax.Parameter::Positional(binder=make_binder(name), ty=None)
      }),
    ),
    ret_ty=None,
    err_ty=@syntax.ErrorType::NoErrorType,
    body=@syntax.DeclBody::DeclBody(
      local_types=to_list([]),
      expr=body,
    ),
    vis=@syntax.Visibility::Default,
    loc=dummy_loc,
    attrs=to_list([]),
    doc=@syntax.DocString::empty(),
  )
}
