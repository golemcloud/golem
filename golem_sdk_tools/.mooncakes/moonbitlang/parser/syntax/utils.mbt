///|
/// MoonBit parsing utilities - translated from OCaml parsing_util.ml
// Translated from src/ast/parsing_util.ml

// Global counter for generating unique variable names
let counter : Ref[Int] = Ref::new(0)

///|
/// Create constant expression with location
pub fn make_constant_expr(loc~ : Location, c : Constant) -> Expr {
  Constant(c~, loc~)
}

///|
/// Create identifier expression
pub fn make_ident_expr(loc~ : Location, id : Var) -> Expr {
  Ident(id~, loc~)
}

///|
/// Create tuple expression
pub fn make_tuple_expr(loc~ : Location, exprs : @list.List[Expr]) -> Expr {
  Tuple(exprs~, loc~)
}

///|
/// Create array expression with possible spread elements
pub fn make_array_expr(
  loc~ : Location,
  elems : @list.List[SpreadableElem],
) -> Expr {
  // Try to convert to regular array if no spread elements
  let mut acc : @list.List[Expr] = @list.empty()
  for elem in elems {
    match elem {
      Regular(e) => acc = acc.add(e)
      Spread(_) => return ArraySpread(elems~, loc~)
    }
  } nobreak {
    return Array(
      // If we have only regular elements, create a simple array
      exprs=acc.rev(),
      loc~,
    )
  }
}

///|
/// Create interpolation elements from literal components
pub fn make_interps(c : @tokens.InterpLiteral) -> @list.List[InterpElem] {
  let result = c.map(fn(lit) {
    match lit {
      InterpLit(repr~, loc~) => Literal(repr~, loc~)
      InterpSource(s) => Source(s)
    }
  })
  @list.from_array(result)
}

///|
/// Create interpolation expression
pub fn make_interp_expr(loc~ : Location, c : @tokens.InterpLiteral) -> Expr {
  let elems = make_interps(c)
  Interp(elems~, loc~)
}

///|
/// Create record expression
pub fn make_record_expr(
  loc~ : Location,
  trailing~ : TrailingMark,
  type_name : TypeName?,
  fields : @list.List[FieldDef],
) -> Expr {
  Record(type_name~, fields~, trailing~, loc~)
}

///|
/// Create alias pattern
pub fn make_alias_pattern(
  loc~ : Location,
  pat : Pattern,
  alias_ : Binder,
) -> Pattern {
  Alias(pat~, alias_~, loc~)
}

///|
/// Create constant pattern
pub fn make_constant_pattern(loc~ : Location, c : Constant) -> Pattern {
  Constant(c~, loc~)
}

///|
/// Create tuple pattern
pub fn make_tuple_pattern(
  loc~ : Location,
  pats : @list.List[Pattern],
) -> Pattern {
  Tuple(pats~, loc~)
}

///|
/// Create constructor pattern
pub fn make_constr_pattern(
  loc~ : Location,
  constr : Constructor,
  args : @list.List[ConstrPatArg]?,
  is_open : Bool,
) -> Pattern {
  Constr(constr~, args~, is_open~, loc~)
}

///|
/// Create tuple type
pub fn make_tuple_type(loc~ : Location, tys : @list.List[Type]) -> Type {
  Tuple(tys~, loc~)
}

///|
/// Create option type
pub fn make_option_type(
  loc~ : Location,
  constr_loc~ : Location,
  ty : Type,
) -> Type {
  Option(ty~, loc~, question_loc=constr_loc)
}

///|
/// Create field pattern
pub fn make_field_pat(
  loc~ : Location,
  label : Label,
  pattern : Pattern,
  is_pun : Bool,
) -> FieldPat {
  FieldPat::{ label, pattern, is_pun, loc }
}

///|
/// Create unary plus expression
pub fn make_uplus(loc~ : Location, name : String, arg : Expr) -> Expr {
  match (name, arg) {
    ("+", Constant(c=Constant::Int(_), loc=_)) => arg
    ("+", Constant(c=Constant::Double(_), loc=_)) => arg
    _ => {
      let op = Var::{ name: Ident(name="~+"), loc }
      Apply(
        func=make_ident_expr(loc~, op),
        args=@list.singleton(Argument::{
          value: arg,
          kind: ArgumentKind::Positional,
        }),
        attr=ApplyAttr::NoAttr,
        loc~,
      )
    }
  }
}

///|
/// Create unary minus expression
pub fn make_uminus(loc~ : Location, name : String, arg : Expr) -> Expr {
  match (name, arg) {
    ("-", Constant(c=Constant::Int(x), loc=_)) => {
      let new_val = match x {
        ['-', .. num] => num.to_string()
        _ => "-" + x
      }
      make_constant_expr(loc~, Constant::Int(new_val))
    }
    ("-", Constant(c=Constant::Int64(x), loc=_)) => {
      let new_val = match x {
        ['-', .. num] => num.to_string()
        _ => "-" + x
      }
      make_constant_expr(loc~, Constant::Int64(new_val))
    }
    ("-", Constant(c=Constant::Double(x), loc=_)) => {
      let new_val = match x {
        ['-', .. num] => num.to_string()
        _ => "-" + x
      }
      make_constant_expr(loc~, Constant::Double(new_val))
    }
    _ => {
      let op = Var::{ name: Ident(name~), loc }
      Unary(op~, expr=arg, loc~)
    }
  }
}

///|
/// Create logical not expression
pub fn make_unot(loc~ : Location, arg : Expr) -> Expr {
  let op = Var::{ name: Ident(name="!"), loc }
  Unary(op~, expr=arg, loc~)
}

///|
pub fn bracket_loc(index : Expr) -> Location {
  let loc = loc_of_expression(index)
  let start = shift_col(loc.start, -1)
  let end = shift_col(loc.end, 1)
  Location::{ start, end }
}

///|
/// Desugar array get operation to method call
pub fn desugar_array_get(loc~ : Location, obj : Expr, index : Expr) -> Expr {
  DotApply(
    self=obj,
    method_name=Label::{
      name: operators_table.get("_[_]").unwrap(),
      loc: bracket_loc(index),
    },
    args=@list.from_array([
      Argument::{ value: index, kind: ArgumentKind::Positional },
    ]),
    return_self=false,
    attr=ApplyAttr::NoAttr,
    loc~,
  )
}

///|
/// Desugar array set operation to method call
pub fn desugar_array_set(
  loc~ : Location,
  obj : Expr,
  index : Expr,
  value : Expr,
) -> Expr {
  DotApply(
    self=obj,
    method_name=Label::{
      name: operators_table.get("_[_]=_").unwrap(),
      loc: bracket_loc(index),
    },
    args=@list.from_array([
      Argument::{ value: index, kind: ArgumentKind::Positional },
      Argument::{ value, kind: ArgumentKind::Positional },
    ]),
    return_self=false,
    attr=ApplyAttr::NoAttr,
    loc~,
  )
}

///|
/// Desugar augmented array assignment
pub fn desugar_array_augmented_set(
  loc : Location,
  op : Var,
  array : Expr,
  index : Expr,
  value : Expr,
) -> Expr {

  // Generate unique variable names for array and index
  // Use a simple counter instead of random for reproducibility
  let arr_name = "*array_" + counter.val.to_string()
  counter.val = counter.val + 1
  let arr_loc = loc_of_expression(array)
  let arr_var = Var::{ name: Ident(name=arr_name), loc: arr_loc }
  let arr_ident = make_ident_expr(loc=arr_loc, arr_var)
  let idx_name = "*index_" + counter.val.to_string()
  counter.val = counter.val + 1
  let idx_loc = loc_of_expression(index)
  let idx_var = Var::{ name: Ident(name=idx_name), loc: idx_loc }
  let idx_ident = make_ident_expr(loc=idx_loc, idx_var)
  Let(
    pattern=Pattern::Var(Binder::{ name: arr_name, loc: no_location }),
    expr=array,
    body=Let(
      pattern=Pattern::Var(Binder::{ name: idx_name, loc: no_location }),
      expr=index,
      body=desugar_array_set(
        loc~,
        arr_ident,
        idx_ident,
        Infix(
          op~,
          lhs=desugar_array_get(loc~, arr_ident, idx_ident),
          rhs=value,
          loc~,
        ),
      ),
      loc~,
    ),
    loc~,
  )
}

///|
/// Try to create assignment expression
pub fn make_assign_opt(loc~ : Location, lhs : Expr, rhs : Expr) -> Expr? {
  match lhs {
    Ident(id~, loc=_) =>
      Some(Assign(var_=id, expr=rhs, augmented_by=None, loc~))
    Field(record~, accessor~, loc=_) =>
      Some(Mutate(record~, accessor~, field=rhs, augmented_by=None, loc~))
    ArrayGet(array~, index~, loc=_) =>
      Some(ArraySet(array~, index~, value=rhs, loc~))
    _ => None
  }
}

///|
/// Try to create augmented assignment expression
pub fn make_augmented_assign_opt(
  loc~ : Location,
  op : Var,
  lhs : Expr,
  rhs : Expr,
) -> Expr? {
  match lhs {
    Ident(id~, loc=_) =>
      Some(Assign(var_=id, expr=rhs, augmented_by=Some(op), loc~))
    Field(record~, accessor~, loc=_) =>
      Some(Mutate(record~, accessor~, field=rhs, augmented_by=Some(op), loc~))
    ArrayGet(array~, index~, loc=_) =>
      Some(ArrayAugmentedSet(op~, array~, index~, value=rhs, loc~))
    _ => None
  }
}

///|
/// Create field definition
pub fn make_field_def(
  loc~ : Location,
  label : Label,
  expr : Expr,
  is_pun : Bool,
) -> FieldDef {
  FieldDef::{ label, expr, is_pun, loc }
}

///|
/// Convert label to expression
pub fn label_to_expr(loc~ : Location, label : Label) -> Expr {
  make_ident_expr(loc~, Var::{ name: Ident(name=label.name), loc })
}

///|
/// Convert label to pattern
pub fn label_to_pat(loc~ : Location, label : Label) -> Pattern {
  Pattern::Var(Binder::{ name: label.name, loc })
}

///|
/// Parse integer literal with suffix
pub fn make_int(i : String) -> Constant {
  if i.has_suffix("L") {
    let base = try! i.sub(end=i.length() - 1)
    if base.has_suffix("U") {
      let u64 = try! base.sub(end=base.length() - 1)
      UInt64(u64.to_string())
    } else {
      Int64(base.to_string())
    }
  } else if i.has_suffix("U") {
    let u32 = try! i.sub(end=i.length() - 1)
    UInt(u32.to_string())
  } else if i.has_suffix("N") {
    let bigint = try! i.sub(end=i.length() - 1)
    BigInt(bigint.to_string())
  } else {
    Int(i)
  }
}

///|
/// Create double constant
pub fn make_double(f : String) -> Constant {
  Constant::Double(f)
}

///|
/// Create float constant
pub fn make_float(f : String) -> Constant {
  if f.has_suffix("F") {
    let base = try! f.sub(end=f.length() - 1)
    Float(base.to_string())
  } else {
    abort("make_float: string must end with F")
  }
}

///|
/// Arrow function parameter type
pub(all) enum ArrowFnParam {
  Named(Binder)
  Unnamed(Location)
}

///|
/// Convert binder to expression
pub fn binder_to_expr(p : Binder) -> Expr {
  make_ident_expr(loc=p.loc, Var::{ name: Ident(name=p.name), loc: p.loc })
}

///|
/// Convert arrow function parameter to expression
pub fn arrow_fn_param_to_expr(p : ArrowFnParam) -> Expr {
  match p {
    Named(b) => binder_to_expr(b)
    Unnamed(loc) => Hole(loc~, kind=Hole::Incomplete)
  }
}

///|
/// Create arrow function
pub fn make_arrow_fn(
  params : @list.List[(ArrowFnParam, Type?)],
  body : Expr,
  params_loc~ : Location,
  loc~ : Location,
) -> Func {
  let parameters = params.map(fn(p) {
    let (param, ty) = p
    match param {
      Named(binder) => Parameter::Positional(binder~, ty~)
      Unnamed(loc) => Parameter::DiscardPositional(ty~, loc~)
    }
  })
  Func::{
    parameters,
    params_loc,
    body,
    return_type: None,
    error_type: ErrorType::NoErrorType,
    kind: FnKind::Arrow,
    has_error: None,
    is_async: None,
    loc,
  }
}

///|
pub fn Expr::loc(self : Self) -> Location {
  self |> loc_of_expression
}

///|
pub fn Type::loc(self : Type) -> Location {
  match self {
    Option(loc~, ..)
    | Name(loc~, ..)
    | Tuple(loc~, ..)
    | Arrow(loc~, ..)
    | Any(loc~)
    | Object({ loc, .. }) => loc
  }
}

// Helper functions that need to be defined elsewhere

///|
pub fn loc_of_expression(expr : Expr) -> Location {
  match expr {
    Apply(func=_, args=_, attr=_, loc~) => loc
    Infix(op=_, lhs=_, rhs=_, loc~) => loc
    Unary(op=_, expr=_, loc~) => loc
    Array(exprs=_, loc~) => loc
    ArraySpread(elems=_, loc~) => loc
    ArrayGet(array=_, index=_, loc~) => loc
    ArrayGetSlice(array=_, start_index=_, end_index=_, index_loc=_, loc~) => loc
    ArraySet(array=_, index=_, value=_, loc~) => loc
    ArrayAugmentedSet(op=_, array=_, index=_, value=_, loc~) => loc
    Constant(c=_, loc~) => loc
    MultilineString(elems=_, loc~) => loc
    Interp(elems=_, loc~) => loc
    Constraint(expr=_, ty=_, loc~) => loc
    Constr(constr=_, loc~) => loc
    While(loop_cond=_, loop_body=_, while_else=_, label=_, loc~) => loc
    Function(func=_, loc~) => loc
    Ident(id=_, loc~) => loc
    If(cond=_, ifso=_, ifnot=_, loc~) => loc
    Guard(cond=_, otherwise=_, body=_, loc~) => loc
    Is(expr=_, pat=_, loc~) => loc
    IsLexMatch(expr=_, strategy=_, pat=_, pat_loc=_, loc~) => loc
    Defer(loc~, ..) => loc
    LetFn(name=_, func=_, body=_, loc~) => loc
    LetRec(bindings=_, body=_, loc~) => loc
    LetAnd(bindings=_, body=_, loc~) => loc
    Let(pattern=_, expr=_, body=_, loc~) => loc
    Sequence(exprs=_, last_expr=_, loc~) => loc
    Tuple(exprs=_, loc~) => loc
    Record(type_name=_, fields=_, trailing=_, loc~) => loc
    RecordUpdate(type_name=_, record=_, fields=_, loc~) => loc
    Field(record=_, accessor=_, loc~) => loc
    Method(type_name=_, method_name=_, loc~) => loc
    DotApply(self=_, method_name=_, args=_, return_self=_, attr=_, loc~) => loc
    As(expr=_, trait_=_, loc~) => loc
    Mutate(record=_, accessor=_, field=_, augmented_by=_, loc~) => loc
    Match(expr=_, cases=_, match_loc=_, loc~) => loc
    LexMatch(strategy=_, expr=_, match_loc=_, cases=_, loc~) => loc
    LetMut(binder=_, ty=_, expr=_, body=_, loc~) => loc
    Pipe(lhs=_, rhs=_, loc~) => loc
    Assign(var_=_, expr=_, augmented_by=_, loc~) => loc
    Hole(loc~, kind=_) => loc
    Return(return_value=_, loc~) => loc
    Raise(err_value=_, loc~) => loc
    Unit(loc~, faked=_) => loc
    Break(arg=_, label=_, loc~) => loc
    Continue(args=_, label=_, loc~) => loc
    Loop(arg=_, body=_, label=_, loop_loc=_, loc~) => loc
    For(
      binders=_,
      condition=_,
      continue_block=_,
      body=_,
      for_else=_,
      label=_,
      loc~,
      where_clause=_
    ) => loc
    ForEach(binders=_, expr=_, body=_, else_block=_, label=_, loc~) => loc
    Try(
      body=_,
      catch_=_,
      catch_all=_,
      try_else=_,
      has_try=_,
      try_loc=_,
      catch_loc=_,
      else_loc=_,
      loc~
    ) => loc
    TryOperator(loc~, ..) => loc
    Map(elems=_, loc~) => loc
    Group(expr=_, group=_, loc~) => loc
    StaticAssert(asserts=_, body=_) => no_location
  }
}

///|
pub fn Pattern::loc(self : Self) -> Location {
  match self {
    Map(loc~, ..)
    | Record(loc~, ..)
    | Var({ loc, .. })
    | Tuple(loc~, ..)
    | Or(loc~, ..)
    | Constr(loc~, ..)
    | Constraint(loc~, ..)
    | Constant(loc~, ..)
    | Array(loc~, ..)
    | Any(loc~)
    | Alias(loc~, ..)
    | Range(loc~, ..)
    | SpecialConstr(loc~, ..) => loc
  }
}

///|
pub fn Impl::loc(self : Self) -> Location {
  match self {
    TopTypeDef({ loc, .. })
    | TopFuncDef(loc~, ..)
    | TopFuncAlias(loc~, ..)
    | TopLetDef(loc~, ..)
    | TopExpr(loc~, ..)
    | TopImplRelation(loc~, ..)
    | TopTest(loc~, ..)
    | TopTrait({ loc, .. })
    | TopBatchTypeAlias(loc~, ..)
    | TopBatchTraitAlias(loc~, ..)
    | TopView(loc~, ..)
    | TopImpl(loc~, ..)
    | TopUsing(loc~, ..) => loc
  }
}

///|
pub fn Parameter::loc(self : Self) -> Location {
  match self {
    Optional(binder~, ty=_, default~) => binder.loc.merge(default.loc())
    Labelled(binder~, ty~)
    | Positional(binder~, ty~)
    | QuestionOptional(binder~, ty~) =>
      match ty {
        None => binder.loc
        Some(ty) => binder.loc.merge(ty.loc())
      }
    DiscardPositional(ty=_, loc~) => loc
  }
}

///|
pub fn AliasTarget::loc(self : AliasTarget) -> Location {
  match self.target {
    None => self.binder.loc
    Some(target) => self.binder.loc.merge(target.loc)
  }
}

///|
fn shift_col(pos : Position, offset : Int) -> Position {
  Position::{
    lnum: pos.lnum,
    fname: pos.fname,
    bol: pos.bol + offset,
    cnum: pos.cnum + offset,
  }
}

// Placeholder types that need to be defined

///|
pub(all) enum InterpolationComponent {
  InterpLit(StringLiteral, Location)
  InterpSource(InterpSource)
}
