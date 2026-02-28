///|
pub fn param_type_to_type_string(pt : ParamType) -> String {
  match pt {
    Simple(name) => name
    Optional(inner) => "\{param_type_to_type_string(inner)}?"
    List(inner) => "Array[\{param_type_to_type_string(inner)}]"
    ResultType(ok, err) =>
      "Result[\{param_type_to_type_string(ok)}, \{param_type_to_type_string(err)}]"
  }
}

///|
fn make_string_expr(s : String) -> @syntax.Expr {
  @syntax.Expr::Constant(c=@syntax.Constant::String(s), loc=dummy_loc)
}

///|
fn make_int_expr(n : Int) -> @syntax.Expr {
  @syntax.Expr::Constant(c=@syntax.Constant::Int(n.to_string()), loc=dummy_loc)
}

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

///|
fn make_apply(func : @syntax.Expr, args : Array[@syntax.Expr]) -> @syntax.Expr {
  @syntax.Expr::Apply(
    func~,
    args=@list.List::from_array(args.map(make_positional_arg)),
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
    method_name=@syntax.Label::{ name: method_name, loc: dummy_loc },
    args=@list.List::from_array(args.map(make_positional_arg)),
    return_self=false,
    attr=@syntax.ApplyAttr::NoAttr,
    loc=dummy_loc,
  )
}

///|
fn make_field(name : String, expr : @syntax.Expr) -> @syntax.FieldDef {
  @syntax.FieldDef::{
    label: @syntax.Label::{ name, loc: dummy_loc },
    expr,
    is_pun: false,
    loc: dummy_loc,
  }
}

///|
fn make_record(fields : Array[@syntax.FieldDef]) -> @syntax.Expr {
  @syntax.Expr::Record(
    type_name=None,
    fields=@list.List::from_array(fields),
    trailing=@syntax.TrailingMark::Comma,
    loc=dummy_loc,
  )
}

///|
fn make_array(elems : Array[@syntax.Expr]) -> @syntax.Expr {
  @syntax.Expr::Array(exprs=@list.List::from_array(elems), loc=dummy_loc)
}

///|
fn make_tuple(elems : Array[@syntax.Expr]) -> @syntax.Expr {
  @syntax.Expr::Tuple(exprs=@list.List::from_array(elems), loc=dummy_loc)
}

///|
fn make_method_ref(type_name : String, method_name : String) -> @syntax.Expr {
  @syntax.Expr::Method(
    type_name=@syntax.TypeName::{
      name: @syntax.LongIdent::Ident(name=type_name),
      is_object: false,
      loc: dummy_loc,
    },
    method_name=@syntax.Label::{ name: method_name, loc: dummy_loc },
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

///|
fn make_binder(name : String) -> @syntax.Binder {
  @syntax.Binder::{ name, loc: dummy_loc }
}

///|
fn param_type_to_ast_type(pt : ParamType) -> @syntax.Type {
  match pt {
    Simple(name) => make_type(name)
    Optional(inner) =>
      @syntax.Type::Option(
        ty=param_type_to_ast_type(inner),
        loc=dummy_loc,
        question_loc=dummy_loc,
      )
    List(inner) =>
      @syntax.Type::Name(
        constr_id=@syntax.ConstrId::{
          id: @syntax.LongIdent::Ident(name="Array"),
          loc: dummy_loc,
        },
        tys=@list.List::from_array([param_type_to_ast_type(inner)]),
        loc=dummy_loc,
      )
    ResultType(ok, err) =>
      @syntax.Type::Name(
        constr_id=@syntax.ConstrId::{
          id: @syntax.LongIdent::Ident(name="Result"),
          loc: dummy_loc,
        },
        tys=@list.List::from_array([
          param_type_to_ast_type(ok),
          param_type_to_ast_type(err),
        ]),
        loc=dummy_loc,
      )
  }
}

///|
fn param_type_to_default_ast_expr(pt : ParamType) -> @syntax.Expr {
  match pt {
    Simple("String") => make_string_expr("")
    Simple("Bool") =>
      @syntax.Expr::Constant(c=@syntax.Constant::Bool(false), loc=dummy_loc)
    Simple("Int") => make_int_expr(0)
    Simple("UInt") =>
      @syntax.Expr::Constant(c=@syntax.Constant::UInt("0"), loc=dummy_loc)
    Simple("Int64") =>
      @syntax.Expr::Constant(c=@syntax.Constant::Int64("0"), loc=dummy_loc)
    Simple("UInt64") =>
      @syntax.Expr::Constant(c=@syntax.Constant::UInt64("0"), loc=dummy_loc)
    Simple("Float") =>
      @syntax.Expr::Constant(c=@syntax.Constant::Float("0.0"), loc=dummy_loc)
    Simple("Double") =>
      @syntax.Expr::Constant(c=@syntax.Constant::Double("0.0"), loc=dummy_loc)
    Simple("Byte") =>
      @syntax.Expr::Constant(c=@syntax.Constant::Byte("\\x00"), loc=dummy_loc)
    Simple("Char") =>
      @syntax.Expr::Constant(c=@syntax.Constant::Char("\\x00"), loc=dummy_loc)
    Simple(_) => make_apply(make_ident_expr("default"), [])
    Optional(_) => make_constr_no_args("None")
    List(_) => make_array([])
    ResultType(ok, _) =>
      make_apply(make_constr_no_args("Ok"), [param_type_to_default_ast_expr(ok)])
  }
}

///|
pub fn param_type_to_schema_expr(pt : ParamType) -> @syntax.Expr {
  let default_expr = param_type_to_default_ast_expr(pt)
  let ty = param_type_to_ast_type(pt)
  let constraint = @syntax.Expr::Constraint(
    expr=default_expr,
    ty~,
    loc=dummy_loc,
  )
  make_apply(make_qualified_expr("schema", "schema_of"), [constraint])
}

///|
fn build_construct_fn(agent : AgentInfo) -> @syntax.Expr {
  let param_count = agent.constructor_params.length()
  // Build final expression: Ok(AgentName::new(p1, p2, ...))
  let new_args : Array[@syntax.Expr] = agent.constructor_params.map(fn(param) {
    let (name, _) = param
    make_ident_expr(name)
  })
  let new_call = make_apply(make_method_ref(agent.name, "new"), new_args)
  let ok_call = make_apply(make_constr_no_args("Ok"), [new_call])
  // Build let bindings for parameters (from last to first)
  let n = agent.constructor_params.length()
  let mut body = ok_call
  for idx in 0..<n {
    let i = n - 1 - idx
    let (name, pt) = agent.constructor_params[i]
    let ty = param_type_to_ast_type(pt)
    let array_get = @syntax.Expr::ArrayGet(
      array=make_ident_expr("elements"),
      index=make_int_expr(i),
      loc=dummy_loc,
    )
    let from_call = make_apply(
      make_qualified_expr("schema", "from_element_value_as"),
      [array_get],
    )
    body = @syntax.Expr::Let(
      pattern=@syntax.Pattern::Constraint(
        pat=@syntax.Pattern::Var(make_binder(name)),
        ty~,
        loc=dummy_loc,
      ),
      expr=from_call,
      body~,
      loc=dummy_loc,
    )
  }
  // Build guard
  let guard_cond = make_infix(
    "==",
    make_dot_apply(make_ident_expr("elements"), "length", []),
    make_int_expr(param_count),
  )
  let error_msg = make_infix(
    "+",
    make_string_expr("Expected \{param_count} elements, got "),
    make_dot_apply(
      make_dot_apply(make_ident_expr("elements"), "length", []),
      "to_string",
      [],
    ),
  )
  let raise_expr = @syntax.Expr::Raise(
    err_value=make_apply(
      make_qualified_constr("common", "AgentError", "InvalidInput"),
      [error_msg],
    ),
    loc=dummy_loc,
  )
  let guard_expr = @syntax.Expr::Guard(
    cond=guard_cond,
    otherwise=Some(raise_expr),
    body~,
    loc=dummy_loc,
  )
  // let elements = @extractor.extract_tuple(input)
  let extract_call = make_apply(
    make_qualified_expr("extractor", "extract_tuple"),
    [make_ident_expr("input")],
  )
  let try_body = @syntax.Expr::Let(
    pattern=@syntax.Pattern::Var(make_binder("elements")),
    expr=extract_call,
    body=guard_expr,
    loc=dummy_loc,
  )
  // catch clause: e => Err(e)
  let catch_case = @syntax.Case::{
    pattern: @syntax.Pattern::Var(make_binder("e")),
    body: make_apply(make_constr_no_args("Err"), [make_ident_expr("e")]),
    guard_: None,
  }
  let try_expr = @syntax.Expr::Try(
    body=try_body,
    catch_=@list.List::from_array([catch_case]),
    catch_all=false,
    try_else=None,
    has_try=true,
    try_loc=dummy_loc,
    catch_loc=dummy_loc,
    else_loc=dummy_loc,
    loc=dummy_loc,
  )
  // fn(input) { try { ... } catch { ... } }
  @syntax.Expr::Function(
    func=@syntax.Func::{
      body: try_expr,
      error_type: @syntax.ErrorType::NoErrorType,
      has_error: None,
      is_async: None,
      kind: @syntax.FnKind::Lambda,
      loc: dummy_loc,
      parameters: @list.List::from_array([
        @syntax.Parameter::Positional(binder=make_binder("input"), ty=None),
      ]),
      params_loc: dummy_loc,
      return_type: None,
    },
    loc=dummy_loc,
  )
}

///|
fn build_register_call(agent : AgentInfo) -> @syntax.Expr {
  // Build input_schema
  let schema_elems : Array[@syntax.Expr] = agent.constructor_params.map(fn(
    param,
  ) {
    let (name, pt) = param
    make_tuple([make_string_expr(name), param_type_to_schema_expr(pt)])
  })
  let input_schema = make_apply(
    make_qualified_constr("common", "DataSchema", "Tuple"),
    [make_array(schema_elems)],
  )
  // Build constructor_ record
  let constructor_record = make_record([
    make_field("name", make_constr_no_args("None")),
    make_field("description", make_string_expr(agent.constructor_description)),
    make_field("prompt_hint", make_constr_no_args("None")),
    make_field("input_schema", input_schema),
  ])
  // Build agent_type record
  let agent_type = make_record([
    make_field("type_name", make_string_expr(agent.name)),
    make_field("description", make_string_expr(agent.description)),
    make_field("constructor_", constructor_record),
    make_field("methods", make_array([])),
    make_field("dependencies", make_array([])),
  ])
  // Build the full record argument
  let record = make_record([
    make_field("name", make_string_expr(agent.name)),
    make_field("agent_type", agent_type),
    make_field("construct", build_construct_fn(agent)),
  ])
  make_apply(make_qualified_expr("agents", "register_agent"), [record])
}

///|
pub fn generate_agent_registration(
  agents : Array[AgentInfo],
) -> @list.List[@syntax.Impl] {
  let body = if agents.length() == 1 {
    build_register_call(agents[0])
  } else {
    let calls : Array[@syntax.Expr] = agents.map(fn(a) {
      build_register_call(a)
    })
    let n = calls.length()
    let init_exprs : Array[@syntax.Expr] = []
    for i in 0..<(n - 1) {
      init_exprs.push(calls[i])
    }
    @syntax.Expr::Sequence(
      exprs=@list.List::from_array(init_exprs),
      last_expr=calls[n - 1],
      loc=dummy_loc,
    )
  }
  @list.List::from_array([
    @syntax.Impl::TopExpr(
      expr=body,
      is_main=false,
      local_types=@list.List::from_array([]),
      is_async=None,
      loc=dummy_loc,
    ),
  ])
}
