///|
struct Identity(Unit)

///|
impl MapVisitor for Identity

///|
test "Pattern" {
  let Identity(_) = Identity(())
  let binder = @syntax.Binder::{ name: "test_binder", loc: dummy_loc }
  let any_pattern = @syntax.Pattern::Any(loc=dummy_loc)
  let int_constant = @syntax.Int("42")
  let constant_pattern = @syntax.Pattern::Constant(
    c=int_constant,
    loc=dummy_loc,
  )
  let int_type = @syntax.Name(
    constr_id=@syntax.ConstrId::{
      id: @syntax.LongIdent::Ident(name="Int"),
      loc: dummy_loc,
    },
    tys=@list.from_array([]),
    loc=dummy_loc,
  )
  let constr_name = @syntax.ConstrName::{
    name: "TestConstructor",
    loc: dummy_loc,
  }
  let constr = @syntax.Constructor::{
    name: constr_name,
    extra_info: @syntax.NoExtraInfo,
    loc: dummy_loc,
  }
  let label = @syntax.Label::{ name: "test_field", loc: dummy_loc }
  let field_pat = @syntax.FieldPat::{
    label,
    pattern: any_pattern,
    is_pun: false,
    loc: dummy_loc,
  }
  let map_elem = @syntax.MapPatElem::{
    key: int_constant,
    pat: any_pattern,
    match_absent: false,
    key_loc: dummy_loc,
    loc: dummy_loc,
  }
  let constr_pat_arg = @syntax.ConstrPatArg::{
    pat: any_pattern,
    kind: @syntax.ArgumentKind::Positional,
  }
  let array_patterns = @syntax.ArrayPatterns::Closed(@list.from_array([]))
  let tuple_pattern = @syntax.Pattern::Tuple(
    pats=@list.from_array([any_pattern, constant_pattern]),
    loc=dummy_loc,
  )
  let any_with_alias = @syntax.Pattern::Alias(
    pat=any_pattern,
    alias_=binder,
    loc=dummy_loc,
  )
  let array_patterns = @syntax.Pattern::Array(
    pats=array_patterns,
    loc=dummy_loc,
  )
  let or_pattern = @syntax.Or(
    pat1=any_pattern,
    pat2=constant_pattern,
    loc=dummy_loc,
  )
  let range_pattern = @syntax.Range(
    lhs=any_pattern,
    rhs=constant_pattern,
    kind=Inclusive,
    loc=dummy_loc,
  )
  let constant_pattern = @syntax.Expr::Constant(c=int_constant, loc=dummy_loc)
  let constraint_pattern = @syntax.Pattern::Constraint(
    pat=any_pattern,
    ty=int_type,
    loc=dummy_loc,
  )
  let constr_pattern = @syntax.Pattern::Constr(
    constr~,
    args=Some(@list.from_array([constr_pat_arg])),
    is_open=false,
    loc=dummy_loc,
  )
  let record_pattern = @syntax.Pattern::Record(
    fields=@list.from_array([field_pat]),
    is_closed=true,
    loc=dummy_loc,
  )
  let map_pattern = @syntax.Pattern::Map(
    elems=@list.from_array([map_elem]),
    is_closed=false,
    loc=dummy_loc,
  )
  assert_eq(
    any_with_alias.json_repr(),
    @syntax.MapVisitorBase::visit_Pattern(
      MapVisitorBase(Identity(())),
      any_with_alias,
    ).json_repr(),
  )
  assert_eq(
    array_patterns.json_repr(),
    @syntax.MapVisitorBase::visit_Pattern(
      MapVisitorBase(Identity(())),
      array_patterns,
    ).json_repr(),
  )
  assert_eq(
    constant_pattern.json_repr(),
    @syntax.MapVisitorBase::visit_Expr(
      MapVisitorBase(Identity(())),
      constant_pattern,
    ).json_repr(),
  )
  assert_eq(
    constraint_pattern.json_repr(),
    @syntax.MapVisitorBase::visit_Pattern(
      MapVisitorBase(Identity(())),
      constraint_pattern,
    ).json_repr(),
  )
  assert_eq(
    constr_pattern.json_repr(),
    @syntax.MapVisitorBase::visit_Pattern(
      MapVisitorBase(Identity(())),
      constr_pattern,
    ).json_repr(),
  )
  assert_eq(
    or_pattern.json_repr(),
    @syntax.MapVisitorBase::visit_Pattern(
      MapVisitorBase(Identity(())),
      or_pattern,
    ).json_repr(),
  )
  assert_eq(
    tuple_pattern.json_repr(),
    @syntax.MapVisitorBase::visit_Pattern(
      MapVisitorBase(Identity(())),
      tuple_pattern,
    ).json_repr(),
  )
  assert_eq(
    record_pattern.json_repr(),
    @syntax.MapVisitorBase::visit_Pattern(
      MapVisitorBase(Identity(())),
      record_pattern,
    ).json_repr(),
  )
  assert_eq(
    map_pattern.json_repr(),
    @syntax.MapVisitorBase::visit_Pattern(
      MapVisitorBase(Identity(())),
      map_pattern,
    ).json_repr(),
  )
  assert_eq(
    range_pattern.json_repr(),
    @syntax.MapVisitorBase::visit_Pattern(
      MapVisitorBase(Identity(())),
      range_pattern,
    ).json_repr(),
  )
}
