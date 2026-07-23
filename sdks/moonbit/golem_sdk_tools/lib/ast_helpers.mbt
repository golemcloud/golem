///|
/// Sentinel position used for generated AST nodes that have no source location.
let dummy_pos : @basic.Position = { fname: "", lnum: 0, bol: 0, cnum: 0 }

///|
/// Sentinel location used for generated AST nodes that have no source location.
let dummy_loc : @basic.Location = { start: dummy_pos, end: dummy_pos }

// --- Primitives ---

///|
/// Creates a `Binder` (variable name binding) with a dummy location.
fn make_binder(name : String) -> @syntax.Binder {
  @syntax.Binder::{ name, loc: dummy_loc }
}

///|
/// Converts an `Array` to a `@list.List`.
fn[T] to_list(arr : Array[T]) -> @list.List[T] {
  @list.List::from_array(arr)
}

// --- Types ---

///|
/// Creates a simple named type AST node.
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

// --- Expressions: Identifiers ---

///|
/// Creates an identifier expression.
fn make_ident_expr(name : String) -> @syntax.Expr {
  @syntax.Expr::Ident(
    id=@syntax.Var::{ name: @syntax.LongIdent::Ident(name~), loc: dummy_loc },
    loc=dummy_loc,
  )
}

///|
/// Creates a package-qualified identifier expression (`@pkg.name`).
fn make_qualified_expr(pkg : String, id : String) -> @syntax.Expr {
  @syntax.Expr::Ident(
    id=@syntax.Var::{ name: @syntax.LongIdent::Dot(pkg~, id~), loc: dummy_loc },
    loc=dummy_loc,
  )
}

// --- Expressions: Application ---

///|
/// Wraps an expression as a positional function argument.
fn make_positional_arg(expr : @syntax.Expr) -> @syntax.Argument {
  @syntax.Argument::{ value: expr, kind: @syntax.ArgumentKind::Positional }
}

///|
/// Creates a function application expression with positional arguments.
fn make_apply(func : @syntax.Expr, args : Array[@syntax.Expr]) -> @syntax.Expr {
  @syntax.Expr::Apply(
    func~,
    args=to_list(args.map(make_positional_arg)),
    attr=@syntax.ApplyAttr::NoAttr,
    loc=dummy_loc,
  )
}

// --- Parameters ---

///|
/// Creates a positional function parameter with a simple type name.
fn make_param(name : String, type_name : String) -> @syntax.Parameter {
  @syntax.Parameter::Positional(
    binder=make_binder(name),
    ty=Some(make_type(type_name)),
  )
}
