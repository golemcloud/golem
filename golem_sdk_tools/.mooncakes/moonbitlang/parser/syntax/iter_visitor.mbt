///|
pub(open) trait IterVisitor {
  base(Self) -> IterVisitorBase[Self] = _

  // Visibility
  visit_Visibility(Self, Visibility) -> Unit = _
  visit_Visibility_Pub(Self, attr~ : String?, loc~ : Location) -> Unit = _
  visit_Visibility_Priv(Self, loc~ : Location) -> Unit = _

  // Constant
  visit_Constant(Self, Constant) -> Unit = _

  // Label
  visit_Label(Self, Label) -> Unit = _

  // ConstrName
  visit_ConstrName(Self, ConstrName) -> Unit = _

  // LongIdent
  visit_LongIdent(Self, LongIdent) -> Unit = _
  visit_LongIdent_Ident(Self, name~ : String) -> Unit = _
  visit_LongIdent_Dot(Self, pkg~ : String, id~ : String) -> Unit = _

  // TypeName
  visit_TypeName(Self, TypeName) -> Unit = _

  // ConstrId
  visit_ConstrId(Self, ConstrId) -> Unit = _

  // Type
  visit_Type(Self, Type) -> Unit = _
  visit_Type_Any(Self, loc~ : Location) -> Unit = _
  visit_Type_Arrow(
    Self,
    args~ : @list.List[Type],
    res~ : Type,
    err~ : ErrorType,
    is_async~ : Location?,
    loc~ : Location,
  ) -> Unit = _
  visit_Type_Name(
    Self,
    constr_id~ : ConstrId,
    tys~ : @list.List[Type],
    loc~ : Location,
  ) -> Unit = _
  visit_Type_Tuple(Self, tys~ : @list.List[Type], loc~ : Location) -> Unit = _
  visit_Type_Option(Self, ty~ : Type, loc~ : Location, question_loc~ : Location) -> Unit = _
  visit_Type_Object(Self, ConstrId) -> Unit = _

  // ErrorType
  visit_ErrorType(Self, ErrorType) -> Unit = _
  visit_ErrorType_ErrorType(Self, ty~ : Type) -> Unit = _
  visit_ErrorType_DefaultErrorType(Self, loc~ : Location) -> Unit = _
  visit_ErrorType_Noraise(Self, loc~ : Location) -> Unit = _
  visit_ErrorType_MaybeError(Self, ty~ : Type) -> Unit = _

  // ConstrParam
  visit_ConstrParam(Self, ConstrParam) -> Unit = _

  // ConstrDecl
  visit_ConstrDecl(Self, ConstrDecl) -> Unit = _

  // ExceptionDecl
  visit_ExceptionDecl(Self, ExceptionDecl) -> Unit = _
  visit_ExceptionDecl_SinglePayload(Self, Type) -> Unit = _
  visit_ExceptionDecl_EnumPayload(Self, @list.List[ConstrDecl]) -> Unit = _

  // FieldName
  visit_FieldName(Self, FieldName) -> Unit = _

  // FieldDecl
  visit_FieldDecl(Self, FieldDecl) -> Unit = _

  // TypeDesc
  visit_TypeDesc(Self, TypeDesc) -> Unit = _
  visit_TypeDesc_Newtype(Self, Type) -> Unit = _
  visit_TypeDesc_Error(Self, ExceptionDecl) -> Unit = _
  visit_TypeDesc_Variant(Self, @list.List[ConstrDecl]) -> Unit = _
  visit_TypeDesc_Record(Self, @list.List[FieldDecl]) -> Unit = _
  visit_TypeDesc_TupleStruct(Self, @list.List[Type]) -> Unit = _
  visit_TypeDesc_Alias(Self, Type) -> Unit = _

  // Hole
  visit_Hole(Self, Hole) -> Unit = _

  // ArgumentKind
  visit_ArgumentKind(Self, ArgumentKind) -> Unit = _
  visit_ArgumentKind_Labelled(Self, Label) -> Unit = _
  visit_ArgumentKind_LabelledPun(Self, Label) -> Unit = _
  visit_ArgumentKind_LabelledOption(
    Self,
    label~ : Label,
    question_loc~ : Location,
  ) -> Unit = _
  visit_ArgumentKind_LabelledOptionPun(
    Self,
    label~ : Label,
    question_loc~ : Location,
  ) -> Unit = _

  // FnKind
  visit_FnKind(Self, FnKind) -> Unit = _

  // Group
  visit_Group(Self, Group) -> Unit = _

  // TrailingMark
  visit_TrailingMark(Self, TrailingMark) -> Unit = _

  // ApplyAttr
  visit_ApplyAttr(Self, ApplyAttr) -> Unit = _

  // TypeVarConstraint
  visit_TypeVarConstraint(Self, TypeVarConstraint) -> Unit = _

  // TypeVarBinder
  visit_TypeVarBinder(Self, TypeVarBinder) -> Unit = _

  // TypeDeclBinder
  visit_TypeDeclBinder(Self, TypeDeclBinder) -> Unit = _

  // Binder
  visit_Binder(Self, Binder) -> Unit = _

  // Var
  visit_Var(Self, Var) -> Unit = _

  // ConstructorExtraInfo
  visit_ConstructorExtraInfo(Self, ConstructorExtraInfo) -> Unit = _
  visit_ConstructorExtraInfo_TypeName(Self, TypeName) -> Unit = _
  visit_ConstructorExtraInfo_Package(Self, String) -> Unit = _

  // Constructor
  visit_Constructor(Self, Constructor) -> Unit = _

  // Accessor
  visit_Accessor(Self, Accessor) -> Unit = _
  visit_Accessor_Label(Self, Label) -> Unit = _
  visit_Accessor_Index(Self, tuple_index~ : Int, loc~ : Location) -> Unit = _
  visit_Accessor_Newtype(Self, loc~ : Location) -> Unit = _

  // AliasTarget
  visit_AliasTarget(Self, AliasTarget) -> Unit = _

  // Argument
  visit_Argument(Self, Argument) -> Unit = _

  // Parameter
  visit_Parameter(Self, Parameter) -> Unit = _
  visit_Parameter_DiscardPositional(Self, ty~ : Type?, loc~ : Location) -> Unit = _
  visit_Parameter_Positional(Self, binder~ : Binder, ty~ : Type?) -> Unit = _
  visit_Parameter_Labelled(Self, binder~ : Binder, ty~ : Type?) -> Unit = _
  visit_Parameter_Optional(Self, binder~ : Binder, default~ : Expr, ty~ : Type?) -> Unit = _
  visit_Parameter_QuestionOptional(Self, binder~ : Binder, ty~ : Type?) -> Unit = _

  // Case
  visit_Case(Self, Case) -> Unit = _

  // LexCase
  visit_LexCase(Self, LexCase) -> Unit = _

  // LexTopPattern
  visit_LexTopPattern(Self, LexTopPattern) -> Unit = _
  visit_LexTopPattern_Pattern(Self, LexPattern) -> Unit = _
  visit_LexTopPattern_Binder(Self, Binder) -> Unit = _
  visit_LexTopPattern_Wildcard(Self, loc~ : Location) -> Unit = _

  // LexPattern
  visit_LexPattern(Self, LexPattern) -> Unit = _
  visit_LexPattern_Regex(Self, lit~ : String, loc~ : Location) -> Unit = _
  visit_LexPattern_RegexInterp(
    Self,
    elems~ : @list.List[InterpElem],
    loc~ : Location,
  ) -> Unit = _
  visit_LexPattern_Alias(
    Self,
    pat~ : LexPattern,
    binder~ : Binder,
    loc~ : Location,
  ) -> Unit = _
  visit_LexPattern_Sequence(
    Self,
    pats~ : @list.List[LexPattern],
    loc~ : Location,
  ) -> Unit = _

  // SpreadableElem
  visit_SpreadableElem(Self, SpreadableElem) -> Unit = _
  visit_SpreadableElem_Regular(Self, Expr) -> Unit = _
  visit_SpreadableElem_Spread(Self, expr~ : Expr, loc~ : Location) -> Unit = _

  // MapExprElem
  visit_MapExprElem(Self, MapExprElem) -> Unit = _

  // StaticAssertion
  visit_StaticAssertion(Self, StaticAssertion) -> Unit = _

  // Func
  visit_Func(Self, Func) -> Unit = _

  // FieldDef
  visit_FieldDef(Self, FieldDef) -> Unit = _

  // InterpElem
  visit_InterpElem(Self, InterpElem) -> Unit = _
  visit_InterpElem_Literal(Self, repr~ : StringLiteral, loc~ : Location) -> Unit = _
  visit_InterpElem_Expr(Self, expr~ : Expr, loc~ : Location) -> Unit = _
  visit_InterpElem_Source(Self, InterpSource) -> Unit = _

  // MultilineStringElem
  visit_MultilineStringElem(Self, MultilineStringElem) -> Unit = _
  visit_MultilineStringElem_String(Self, String) -> Unit = _
  visit_MultilineStringElem_Interp(Self, @list.List[InterpElem]) -> Unit = _

  // TryOperatorKind
  visit_TryOperatorKind(Self, TryOperatorKind) -> Unit = _

  // DotDotBinder
  visit_DotDotBinder(Self, DotDotBinder) -> Unit = _
  visit_DotDotBinder_BinderAs(Self, Binder) -> Unit = _
  visit_DotDotBinder_Binder(Self, Binder) -> Unit = _

  // ArrayPattern
  visit_ArrayPattern(Self, ArrayPattern) -> Unit = _
  visit_ArrayPattern_Pattern(Self, Pattern) -> Unit = _
  visit_ArrayPattern_StringSpread(Self, str~ : StringLiteral, loc~ : Location) -> Unit = _
  visit_ArrayPattern_BytesSpread(Self, bytes~ : BytesLiteral, loc~ : Location) -> Unit = _
  visit_ArrayPattern_ConstSpread(
    Self,
    binder~ : Binder,
    pkg~ : String?,
    loc~ : Location,
  ) -> Unit = _

  // ArrayPatterns
  visit_ArrayPatterns(Self, ArrayPatterns) -> Unit = _
  visit_ArrayPatterns_Closed(Self, @list.List[ArrayPattern]) -> Unit = _
  visit_ArrayPatterns_Open(
    Self,
    @list.List[ArrayPattern],
    @list.List[ArrayPattern],
    DotDotBinder,
  ) -> Unit = _

  // WhereClause
  visit_WhereClause(Self, WhereClause) -> Unit = _

  // FieldPat
  visit_FieldPat(Self, FieldPat) -> Unit = _

  // ConstrPatArg
  visit_ConstrPatArg(Self, ConstrPatArg) -> Unit = _

  // MapPatElem
  visit_MapPatElem(Self, MapPatElem) -> Unit = _

  // RangeKind
  visit_RangeKind(Self, RangeKind) -> Unit = _

  // Pattern
  visit_Pattern(Self, Pattern) -> Unit = _
  visit_Pattern_Alias(Self, pat~ : Pattern, alias_~ : Binder, loc~ : Location) -> Unit = _
  visit_Pattern_Any(Self, loc~ : Location) -> Unit = _
  visit_Pattern_Array(Self, pats~ : ArrayPatterns, loc~ : Location) -> Unit = _
  visit_Pattern_Constant(Self, c~ : Constant, loc~ : Location) -> Unit = _
  visit_Pattern_Constraint(Self, pat~ : Pattern, ty~ : Type, loc~ : Location) -> Unit = _
  visit_Pattern_Constr(
    Self,
    constr~ : Constructor,
    args~ : @list.List[ConstrPatArg]?,
    is_open~ : Bool,
    loc~ : Location,
  ) -> Unit = _
  visit_Pattern_Or(Self, pat1~ : Pattern, pat2~ : Pattern, loc~ : Location) -> Unit = _
  visit_Pattern_Tuple(Self, pats~ : @list.List[Pattern], loc~ : Location) -> Unit = _
  visit_Pattern_Var(Self, Binder) -> Unit = _
  visit_Pattern_Record(
    Self,
    fields~ : @list.List[FieldPat],
    is_closed~ : Bool,
    loc~ : Location,
  ) -> Unit = _
  visit_Pattern_Map(
    Self,
    elems~ : @list.List[MapPatElem],
    is_closed~ : Bool,
    loc~ : Location,
  ) -> Unit = _
  visit_Pattern_Range(
    Self,
    lhs~ : Pattern,
    rhs~ : Pattern,
    kind~ : RangeKind,
    loc~ : Location,
  ) -> Unit = _
  visit_Pattern_SpecialConstr(
    Self,
    binder~ : Binder,
    args~ : @list.List[ConstrPatArg],
    loc~ : Location,
  ) -> Unit = _

  // LocalTypeDecl
  visit_LocalTypeDecl(Self, LocalTypeDecl) -> Unit = _

  // DerivingDirective
  visit_DerivingDirective(Self, DerivingDirective) -> Unit = _

  // TypeDecl
  visit_TypeDecl(Self, TypeDecl) -> Unit = _

  // FuncStubs
  visit_FuncStubs(Self, FuncStubs) -> Unit = _
  visit_FuncStubs_Import(
    Self,
    module_name~ : StringLiteral,
    func_name~ : StringLiteral,
    language~ : StringLiteral?,
  ) -> Unit = _
  visit_FuncStubs_Embedded(
    Self,
    language~ : StringLiteral?,
    code~ : EmbeddedCode,
  ) -> Unit = _

  // EmbeddedCode
  visit_EmbeddedCode(Self, EmbeddedCode) -> Unit = _
  visit_EmbeddedCode_CodeString(Self, StringLiteral) -> Unit = _
  visit_EmbeddedCode_CodeMultilineString(Self, @list.List[String]) -> Unit = _

  // DeclBody
  visit_DeclBody(Self, DeclBody) -> Unit = _
  visit_DeclBody_DeclBody(
    Self,
    local_types~ : @list.List[LocalTypeDecl],
    expr~ : Expr,
  ) -> Unit = _
  visit_DeclBody_DeclStubs(Self, FuncStubs) -> Unit = _

  // FuncDecl
  visit_FunDecl(Self, FunDecl) -> Unit = _

  // TraitMethodDecl
  visit_TraitMethodDecl(Self, TraitMethodDecl) -> Unit = _

  // TraitDecl
  visit_TraitDecl(Self, TraitDecl) -> Unit = _

  // Impl
  visit_Impl(Self, Impl) -> Unit = _
  visit_Impl_TopExpr(
    Self,
    expr~ : Expr,
    is_main~ : Bool,
    local_types~ : @list.List[LocalTypeDecl],
    is_async~ : Location?,
    loc~ : Location,
  ) -> Unit = _
  visit_Impl_TopTest(
    Self,
    expr~ : Expr,
    name~ : TestName,
    params~ : Parameters?,
    local_types~ : @list.List[LocalTypeDecl],
    is_async~ : Location?,
    loc~ : Location,
    attrs~ : @list.List[Attribute],
    doc~ : DocString,
  ) -> Unit = _
  visit_Impl_TopTypeDef(Self, TypeDecl) -> Unit = _
  visit_Impl_TopFuncDef(
    Self,
    fun_decl~ : FunDecl,
    decl_body~ : DeclBody,
    loc~ : Location,
  ) -> Unit = _
  visit_Impl_TopFuncAlias(
    Self,
    pkg~ : Label?,
    type_name~ : Label?,
    targets~ : @list.List[AliasTarget],
    vis~ : Visibility,
    attrs~ : @list.List[Attribute],
    is_list~ : Bool,
    doc~ : DocString,
    loc~ : Location,
  ) -> Unit = _
  visit_Impl_TopLetDef(
    Self,
    binder~ : Binder,
    ty~ : Type?,
    expr~ : Expr,
    vis~ : Visibility,
    is_constant~ : Bool,
    loc~ : Location,
    attrs~ : @list.List[Attribute],
    doc~ : DocString,
    is_declare~ : Bool,
  ) -> Unit = _
  visit_Impl_TopTrait(Self, TraitDecl) -> Unit = _
  visit_Impl_TopBatchTypeAlias(
    Self,
    pkg~ : Label?,
    targets~ : @list.List[AliasTarget],
    vis~ : Visibility,
    loc~ : Location,
    attrs~ : @list.List[Attribute],
    is_list~ : Bool,
    doc~ : DocString,
  ) -> Unit = _
  visit_Impl_TopBatchTraitAlias(
    Self,
    pkg~ : Label?,
    targets~ : @list.List[AliasTarget],
    vis~ : Visibility,
    loc~ : Location,
    attrs~ : @list.List[Attribute],
    is_list~ : Bool,
    doc~ : DocString,
  ) -> Unit = _
  visit_Impl_TopImpl(
    Self,
    self_ty~ : Type?,
    trait_~ : TypeName,
    method_name~ : Binder,
    has_error~ : Location?,
    quantifiers~ : @list.List[TypeVarBinder],
    params~ : Parameters,
    ret_ty~ : Type?,
    err_ty~ : ErrorType,
    body~ : DeclBody,
    vis~ : Visibility,
    loc~ : Location,
    attrs~ : @list.List[Attribute],
    doc~ : DocString,
  ) -> Unit = _
  visit_Impl_TopView(
    Self,
    quantifiers~ : @list.List[TypeVarBinder],
    source_ty~ : Type,
    view_type_name~ : String,
    view_type_loc~ : Location,
    view_constrs~ : @list.List[ConstrDecl],
    view_func_name~ : Binder,
    parameters~ : Parameters,
    params_loc~ : Location,
    body~ : Expr,
    vis~ : Visibility,
    loc~ : Location,
    attrs~ : @list.List[Attribute],
    doc~ : DocString,
  ) -> Unit = _
  visit_Impl_TopImplRelation(
    Self,
    self_ty~ : Type,
    trait_~ : TypeName,
    quantifiers~ : @list.List[TypeVarBinder],
    vis~ : Visibility,
    attrs~ : @list.List[Attribute],
    loc~ : Location,
    doc~ : DocString,
    is_declare~ : Bool,
  ) -> Unit = _
  visit_Impl_TopUsing(
    Self,
    pkg~ : Label,
    names~ : @list.List[(AliasTarget, UsingKind)],
    vis~ : Visibility,
    attrs~ : @list.List[Attribute],
    loc~ : Location,
    doc~ : DocString,
  ) -> Unit = _

  // UsingKind
  visit_UsingKind(Self, UsingKind) -> Unit = _

  // Expr
  visit_Expr(Self, Expr) -> Unit = _
  visit_Expr_Apply(
    Self,
    func~ : Expr,
    args~ : @list.List[Argument],
    attr~ : ApplyAttr,
    loc~ : Location,
  ) -> Unit = _
  visit_Expr_Infix(Self, op~ : Var, lhs~ : Expr, rhs~ : Expr, loc~ : Location) -> Unit = _
  visit_Expr_Unary(Self, op~ : Var, expr~ : Expr, loc~ : Location) -> Unit = _
  visit_Expr_Array(Self, exprs~ : @list.List[Expr], loc~ : Location) -> Unit = _
  visit_Expr_ArraySpread(
    Self,
    elems~ : @list.List[SpreadableElem],
    loc~ : Location,
  ) -> Unit = _
  visit_Expr_ArrayGet(Self, array~ : Expr, index~ : Expr, loc~ : Location) -> Unit = _
  visit_Expr_ArrayGetSlice(
    Self,
    array~ : Expr,
    start_index~ : Expr?,
    end_index~ : Expr?,
    index_loc~ : Location,
    loc~ : Location,
  ) -> Unit = _
  visit_Expr_ArraySet(
    Self,
    array~ : Expr,
    index~ : Expr,
    value~ : Expr,
    loc~ : Location,
  ) -> Unit = _
  visit_Expr_ArrayAugmentedSet(
    Self,
    op~ : Var,
    array~ : Expr,
    index~ : Expr,
    value~ : Expr,
    loc~ : Location,
  ) -> Unit = _
  visit_Expr_Constant(Self, c~ : Constant, loc~ : Location) -> Unit = _
  visit_Expr_MultilineString(
    Self,
    elems~ : @list.List[MultilineStringElem],
    loc~ : Location,
  ) -> Unit = _
  visit_Expr_Interp(Self, elems~ : @list.List[InterpElem], loc~ : Location) -> Unit = _
  visit_Expr_Constraint(Self, expr~ : Expr, ty~ : Type, loc~ : Location) -> Unit = _
  visit_Expr_Constr(Self, constr~ : Constructor, loc~ : Location) -> Unit = _
  visit_Expr_While(
    Self,
    loop_cond~ : Expr,
    loop_body~ : Expr,
    while_else~ : Expr?,
    label~ : Label?,
    loc~ : Location,
  ) -> Unit = _
  visit_Expr_Function(Self, func~ : Func, loc~ : Location) -> Unit = _
  visit_Expr_Ident(Self, id~ : Var, loc~ : Location) -> Unit = _
  visit_Expr_If(
    Self,
    cond~ : Expr,
    ifso~ : Expr,
    ifnot~ : Expr?,
    loc~ : Location,
  ) -> Unit = _
  visit_Expr_Guard(
    Self,
    cond~ : Expr,
    otherwise~ : Expr?,
    body~ : Expr,
    loc~ : Location,
  ) -> Unit = _
  visit_Expr_Is(Self, expr~ : Expr, pat~ : Pattern, loc~ : Location) -> Unit = _
  visit_Expr_IsLexMatch(
    Self,
    expr~ : Expr,
    strategy~ : Label?,
    pat~ : @list.List[LexTopPattern],
    pat_loc~ : Location,
    loc~ : Location,
  ) -> Unit = _
  visit_Expr_Defer(Self, expr~ : Expr, body~ : Expr, loc~ : Location) -> Unit = _
  visit_Expr_LetFn(
    Self,
    name~ : Binder,
    func~ : Func,
    body~ : Expr,
    loc~ : Location,
  ) -> Unit = _
  visit_Expr_LetRec(
    Self,
    bindings~ : @list.List[(Binder, Func)],
    body~ : Expr,
    loc~ : Location,
  ) -> Unit = _
  visit_Expr_LetAnd(
    Self,
    bindings~ : @list.List[(Binder, Type?, Func)],
    body~ : Expr,
    loc~ : Location,
  ) -> Unit = _
  visit_Expr_Let(
    Self,
    pattern~ : Pattern,
    expr~ : Expr,
    body~ : Expr,
    loc~ : Location,
  ) -> Unit = _
  visit_Expr_Sequence(
    Self,
    exprs~ : @list.List[Expr],
    last_expr~ : Expr,
    loc~ : Location,
  ) -> Unit = _
  visit_Expr_Tuple(Self, exprs~ : @list.List[Expr], loc~ : Location) -> Unit = _
  visit_Expr_Record(
    Self,
    type_name~ : TypeName?,
    fields~ : @list.List[FieldDef],
    trailing~ : TrailingMark,
    loc~ : Location,
  ) -> Unit = _
  visit_Expr_RecordUpdate(
    Self,
    type_name~ : TypeName?,
    record~ : Expr,
    fields~ : @list.List[FieldDef],
    loc~ : Location,
  ) -> Unit = _
  visit_Expr_Field(Self, record~ : Expr, accessor~ : Accessor, loc~ : Location) -> Unit = _
  visit_Expr_Method(
    Self,
    type_name~ : TypeName,
    method_name~ : Label,
    loc~ : Location,
  ) -> Unit = _
  visit_Expr_DotApply(
    Self,
    self~ : Expr,
    method_name~ : Label,
    args~ : @list.List[Argument],
    return_self~ : Bool,
    attr~ : ApplyAttr,
    loc~ : Location,
  ) -> Unit = _
  visit_Expr_As(Self, expr~ : Expr, trait_~ : TypeName, loc~ : Location) -> Unit = _
  visit_Expr_Mutate(
    Self,
    record~ : Expr,
    accessor~ : Accessor,
    field~ : Expr,
    augmented_by~ : Var?,
    loc~ : Location,
  ) -> Unit = _
  visit_Expr_Match(
    Self,
    expr~ : Expr,
    cases~ : @list.List[Case],
    match_loc~ : Location,
    loc~ : Location,
  ) -> Unit = _
  visit_Expr_LexMatch(
    Self,
    strategy~ : Label?,
    expr~ : Expr,
    match_loc~ : Location,
    cases~ : @list.List[LexCase],
    loc~ : Location,
  ) -> Unit = _
  visit_Expr_LetMut(
    Self,
    binder~ : Binder,
    ty~ : Type?,
    expr~ : Expr,
    body~ : Expr,
    loc~ : Location,
  ) -> Unit = _
  visit_Expr_Pipe(Self, lhs~ : Expr, rhs~ : Expr, loc~ : Location) -> Unit = _
  visit_Expr_Assign(
    Self,
    var_~ : Var,
    expr~ : Expr,
    augmented_by~ : Var?,
    loc~ : Location,
  ) -> Unit = _
  visit_Expr_Hole(Self, loc~ : Location, kind~ : Hole) -> Unit = _
  visit_Expr_Return(Self, return_value~ : Expr?, loc~ : Location) -> Unit = _
  visit_Expr_Raise(Self, err_value~ : Expr, loc~ : Location) -> Unit = _
  visit_Expr_Unit(Self, loc~ : Location, faked~ : Bool) -> Unit = _
  visit_Expr_Break(Self, arg~ : Expr?, label~ : Label?, loc~ : Location) -> Unit = _
  visit_Expr_Continue(
    Self,
    args~ : @list.List[Expr],
    label~ : Label?,
    loc~ : Location,
  ) -> Unit = _
  visit_Expr_Loop(
    Self,
    arg~ : Expr,
    body~ : @list.List[Case],
    label~ : Label?,
    loop_loc~ : Location,
    loc~ : Location,
  ) -> Unit = _
  visit_Expr_For(
    Self,
    binders~ : @list.List[(Binder, Expr)],
    condition~ : Expr?,
    continue_block~ : @list.List[(Binder, Expr)],
    body~ : Expr,
    for_else~ : Expr?,
    label~ : Label?,
    loc~ : Location,
    where_clause~ : WhereClause?,
  ) -> Unit = _
  visit_Expr_ForEach(
    Self,
    binders~ : @list.List[Binder?],
    expr~ : Expr,
    body~ : Expr,
    else_block~ : Expr?,
    label~ : Label?,
    loc~ : Location,
  ) -> Unit = _
  visit_Expr_Try(
    Self,
    body~ : Expr,
    catch_~ : @list.List[Case],
    catch_all~ : Bool,
    try_else~ : @list.List[Case]?,
    has_try~ : Bool,
    try_loc~ : Location,
    catch_loc~ : Location,
    else_loc~ : Location,
    loc~ : Location,
  ) -> Unit = _
  visit_Expr_TryOperator(
    Self,
    body~ : Expr,
    kind~ : TryOperatorKind,
    try_loc~ : Location,
    loc~ : Location,
  ) -> Unit = _
  visit_Expr_Map(Self, elems~ : @list.List[MapExprElem], loc~ : Location) -> Unit = _
  visit_Expr_Group(Self, expr~ : Expr, group~ : Group, loc~ : Location) -> Unit = _
  visit_Expr_StaticAssert(
    Self,
    asserts~ : @list.List[StaticAssertion],
    body~ : Expr,
  ) -> Unit = _
}

///|
impl IterVisitor with base(self) -> IterVisitorBase[Self] {
  IterVisitorBase::IterVisitorBase(self)
}

///|
impl IterVisitor with visit_Impl(env, impl_) {
  IterVisitorBase::visit_Impl(env, impl_)
}

///|
impl IterVisitor with visit_Impl_TopExpr(
  env,
  expr~,
  is_main~,
  local_types~,
  is_async~,
  loc~,
) {
  IterVisitorBase::visit_Impl_TopExpr(
    env,
    expr~,
    is_main~,
    local_types~,
    is_async~,
    loc~,
  )
}

///|
impl IterVisitor with visit_Impl_TopTest(
  env,
  expr~,
  name~,
  params~,
  local_types~,
  is_async~,
  loc~,
  attrs~,
  doc~,
) {
  IterVisitorBase::visit_Impl_TopTest(
    env,
    expr~,
    name~,
    params~,
    local_types~,
    is_async~,
    loc~,
    attrs~,
    doc~,
  )
}

///|
impl IterVisitor with visit_Impl_TopTypeDef(env, type_decl) {
  IterVisitorBase::visit_Impl_TopTypeDef(env, type_decl)
}

///|
impl IterVisitor with visit_Impl_TopFuncDef(env, fun_decl~, decl_body~, loc~) {
  IterVisitorBase::visit_Impl_TopFuncDef(env, fun_decl~, decl_body~, loc~)
}

///|
impl IterVisitor with visit_Impl_TopFuncAlias(
  env,
  pkg~,
  type_name~,
  targets~,
  vis~,
  attrs~,
  is_list~,
  doc~,
  loc~,
) {
  IterVisitorBase::visit_Impl_TopFuncAlias(
    env,
    pkg~,
    type_name~,
    targets~,
    vis~,
    attrs~,
    is_list~,
    doc~,
    loc~,
  )
}

///|
impl IterVisitor with visit_Impl_TopLetDef(
  env,
  binder~,
  ty~,
  expr~,
  vis~,
  is_constant~,
  loc~,
  attrs~,
  doc~,
  is_declare~,
) {
  IterVisitorBase::visit_Impl_TopLetDef(
    env,
    binder~,
    ty~,
    expr~,
    vis~,
    is_constant~,
    loc~,
    attrs~,
    doc~,
    is_declare~,
  )
}

///|
impl IterVisitor with visit_Impl_TopTrait(env, trait_decl) {
  IterVisitorBase::visit_Impl_TopTrait(env, trait_decl)
}

///|
impl IterVisitor with visit_Impl_TopBatchTypeAlias(
  env,
  pkg~,
  targets~,
  vis~,
  loc~,
  attrs~,
  is_list~,
  doc~,
) {
  IterVisitorBase::visit_Impl_TopBatchTypeAlias(
    env,
    pkg~,
    targets~,
    vis~,
    loc~,
    attrs~,
    is_list~,
    doc~,
  )
}

///|
impl IterVisitor with visit_Impl_TopBatchTraitAlias(
  env,
  pkg~,
  targets~,
  vis~,
  loc~,
  attrs~,
  is_list~,
  doc~,
) {
  IterVisitorBase::visit_Impl_TopBatchTraitAlias(
    env,
    pkg~,
    targets~,
    vis~,
    loc~,
    attrs~,
    is_list~,
    doc~,
  )
}

///|
impl IterVisitor with visit_Impl_TopImpl(
  env,
  self_ty~,
  trait_~,
  method_name~,
  has_error~,
  quantifiers~,
  params~,
  ret_ty~,
  err_ty~,
  body~,
  vis~,
  loc~,
  attrs~,
  doc~,
) {
  IterVisitorBase::visit_Impl_TopImpl(
    env,
    self_ty~,
    trait_~,
    method_name~,
    has_error~,
    quantifiers~,
    params~,
    ret_ty~,
    err_ty~,
    body~,
    vis~,
    loc~,
    attrs~,
    doc~,
  )
}

///|
impl IterVisitor with visit_Impl_TopView(
  env,
  quantifiers~,
  source_ty~,
  view_type_name~,
  view_type_loc~,
  view_constrs~,
  view_func_name~,
  parameters~,
  params_loc~,
  body~,
  vis~,
  loc~,
  attrs~,
  doc~,
) {
  IterVisitorBase::visit_Impl_TopView(
    env,
    quantifiers~,
    source_ty~,
    view_type_name~,
    view_type_loc~,
    view_constrs~,
    view_func_name~,
    parameters~,
    params_loc~,
    body~,
    vis~,
    loc~,
    attrs~,
    doc~,
  )
}

///|
impl IterVisitor with visit_Impl_TopImplRelation(
  env,
  self_ty~,
  trait_~,
  quantifiers~,
  vis~,
  attrs~,
  loc~,
  doc~,
  is_declare~,
) {
  IterVisitorBase::visit_Impl_TopImplRelation(
    env,
    self_ty~,
    trait_~,
    quantifiers~,
    vis~,
    attrs~,
    loc~,
    doc~,
    is_declare~,
  )
}

///|
impl IterVisitor with visit_Impl_TopUsing(
  env,
  pkg~,
  names~,
  vis~,
  attrs~,
  loc~,
  doc~,
) {
  IterVisitorBase::visit_Impl_TopUsing(
    env,
    pkg~,
    names~,
    vis~,
    attrs~,
    loc~,
    doc~,
  )
}

///|
impl IterVisitor with visit_UsingKind(env, using_kind) {
  IterVisitorBase::visit_UsingKind(env, using_kind)
}

///|
impl IterVisitor with visit_Expr(env, expr) {
  IterVisitorBase::visit_Expr(env, expr)
}

///|
impl IterVisitor with visit_Expr_Apply(env, func~, args~, attr~, loc~) {
  IterVisitorBase::visit_Expr_Apply(env, func~, args~, attr~, loc~)
}

///|
impl IterVisitor with visit_Expr_Infix(env, op~, lhs~, rhs~, loc~) {
  IterVisitorBase::visit_Expr_Infix(env, op~, lhs~, rhs~, loc~)
}

///|
impl IterVisitor with visit_Expr_Unary(env, op~, expr~, loc~) {
  IterVisitorBase::visit_Expr_Unary(env, op~, expr~, loc~)
}

///|
impl IterVisitor with visit_Expr_Array(env, exprs~, loc~) {
  IterVisitorBase::visit_Expr_Array(env, exprs~, loc~)
}

///|
impl IterVisitor with visit_Expr_ArraySpread(env, elems~, loc~) {
  IterVisitorBase::visit_Expr_ArraySpread(env, elems~, loc~)
}

///|
impl IterVisitor with visit_Expr_ArrayGet(env, array~, index~, loc~) {
  IterVisitorBase::visit_Expr_ArrayGet(env, array~, index~, loc~)
}

///|
impl IterVisitor with visit_Expr_ArrayGetSlice(
  env,
  array~,
  start_index~,
  end_index~,
  index_loc~,
  loc~,
) {
  IterVisitorBase::visit_Expr_ArrayGetSlice(
    env,
    array~,
    start_index~,
    end_index~,
    index_loc~,
    loc~,
  )
}

///|
impl IterVisitor with visit_Expr_ArraySet(env, array~, index~, value~, loc~) {
  IterVisitorBase::visit_Expr_ArraySet(env, array~, index~, value~, loc~)
}

///|
impl IterVisitor with visit_Expr_ArrayAugmentedSet(
  env,
  op~,
  array~,
  index~,
  value~,
  loc~,
) {
  IterVisitorBase::visit_Expr_ArrayAugmentedSet(
    env,
    op~,
    array~,
    index~,
    value~,
    loc~,
  )
}

///|
impl IterVisitor with visit_Expr_Constant(env, c~, loc~) {
  IterVisitorBase::visit_Expr_Constant(env, c~, loc~)
}

///|
impl IterVisitor with visit_Expr_MultilineString(env, elems~, loc~) {
  IterVisitorBase::visit_Expr_MultilineString(env, elems~, loc~)
}

///|
impl IterVisitor with visit_Expr_Interp(env, elems~, loc~) {
  IterVisitorBase::visit_Expr_Interp(env, elems~, loc~)
}

///|
impl IterVisitor with visit_Expr_Constraint(env, expr~, ty~, loc~) {
  IterVisitorBase::visit_Expr_Constraint(env, expr~, ty~, loc~)
}

///|
impl IterVisitor with visit_Expr_Constr(env, constr~, loc~) {
  IterVisitorBase::visit_Expr_Constr(env, constr~, loc~)
}

///|
impl IterVisitor with visit_Expr_While(
  env,
  loop_cond~,
  loop_body~,
  while_else~,
  label~,
  loc~,
) {
  IterVisitorBase::visit_Expr_While(
    env,
    loop_cond~,
    loop_body~,
    while_else~,
    label~,
    loc~,
  )
}

///|
impl IterVisitor with visit_Expr_Function(env, func~, loc~) {
  IterVisitorBase::visit_Expr_Function(env, func~, loc~)
}

///|
impl IterVisitor with visit_Expr_Ident(env, id~, loc~) {
  IterVisitorBase::visit_Expr_Ident(env, id~, loc~)
}

///|
impl IterVisitor with visit_Expr_If(env, cond~, ifso~, ifnot~, loc~) {
  IterVisitorBase::visit_Expr_If(env, cond~, ifso~, ifnot~, loc~)
}

///|
impl IterVisitor with visit_Expr_Guard(env, cond~, otherwise~, body~, loc~) {
  IterVisitorBase::visit_Expr_Guard(env, cond~, otherwise~, body~, loc~)
}

///|
impl IterVisitor with visit_Expr_Is(env, expr~, pat~, loc~) {
  IterVisitorBase::visit_Expr_Is(env, expr~, pat~, loc~)
}

///|
impl IterVisitor with visit_Expr_Defer(env, expr~, body~, loc~) {
  IterVisitorBase::visit_Expr_Defer(env, expr~, body~, loc~)
}

///|
impl IterVisitor with visit_Expr_LetFn(env, name~, func~, body~, loc~) {
  IterVisitorBase::visit_Expr_LetFn(env, name~, func~, body~, loc~)
}

///|
impl IterVisitor with visit_Expr_LetRec(env, bindings~, body~, loc~) {
  IterVisitorBase::visit_Expr_LetRec(env, bindings~, body~, loc~)
}

///|
impl IterVisitor with visit_Expr_LetAnd(env, bindings~, body~, loc~) {
  IterVisitorBase::visit_Expr_LetAnd(env, bindings~, body~, loc~)
}

///|
impl IterVisitor with visit_Expr_Let(env, pattern~, expr~, body~, loc~) {
  IterVisitorBase::visit_Expr_Let(env, pattern~, expr~, body~, loc~)
}

///|
impl IterVisitor with visit_Expr_Sequence(env, exprs~, last_expr~, loc~) {
  IterVisitorBase::visit_Expr_Sequence(env, exprs~, last_expr~, loc~)
}

///|
impl IterVisitor with visit_Expr_Tuple(env, exprs~, loc~) {
  IterVisitorBase::visit_Expr_Tuple(env, exprs~, loc~)
}

///|
impl IterVisitor with visit_Expr_Record(
  env,
  type_name~,
  fields~,
  trailing~,
  loc~,
) {
  IterVisitorBase::visit_Expr_Record(env, type_name~, fields~, trailing~, loc~)
}

///|
impl IterVisitor with visit_Expr_RecordUpdate(
  env,
  type_name~,
  record~,
  fields~,
  loc~,
) {
  IterVisitorBase::visit_Expr_RecordUpdate(
    env,
    type_name~,
    record~,
    fields~,
    loc~,
  )
}

///|
impl IterVisitor with visit_Expr_Field(env, record~, accessor~, loc~) {
  IterVisitorBase::visit_Expr_Field(env, record~, accessor~, loc~)
}

///|
impl IterVisitor with visit_Expr_Method(env, type_name~, method_name~, loc~) {
  IterVisitorBase::visit_Expr_Method(env, type_name~, method_name~, loc~)
}

///|
impl IterVisitor with visit_Expr_DotApply(
  env,
  self~,
  method_name~,
  args~,
  return_self~,
  attr~,
  loc~,
) {
  IterVisitorBase::visit_Expr_DotApply(
    env,
    self~,
    method_name~,
    args~,
    return_self~,
    attr~,
    loc~,
  )
}

///|
impl IterVisitor with visit_Expr_As(env, expr~, trait_~, loc~) {
  IterVisitorBase::visit_Expr_As(env, expr~, trait_~, loc~)
}

///|
impl IterVisitor with visit_Expr_Mutate(
  env,
  record~,
  accessor~,
  field~,
  augmented_by~,
  loc~,
) {
  IterVisitorBase::visit_Expr_Mutate(
    env,
    record~,
    accessor~,
    field~,
    augmented_by~,
    loc~,
  )
}

///|
impl IterVisitor with visit_Expr_Match(env, expr~, cases~, match_loc~, loc~) {
  IterVisitorBase::visit_Expr_Match(env, expr~, cases~, match_loc~, loc~)
}

///|
impl IterVisitor with visit_Expr_LetMut(env, binder~, ty~, expr~, body~, loc~) {
  IterVisitorBase::visit_Expr_LetMut(env, binder~, ty~, expr~, body~, loc~)
}

///|
impl IterVisitor with visit_Expr_Pipe(env, lhs~, rhs~, loc~) {
  IterVisitorBase::visit_Expr_Pipe(env, lhs~, rhs~, loc~)
}

///|
impl IterVisitor with visit_Expr_Assign(env, var_~, expr~, augmented_by~, loc~) {
  IterVisitorBase::visit_Expr_Assign(env, var_~, expr~, augmented_by~, loc~)
}

///|
impl IterVisitor with visit_Expr_Hole(env, loc~, kind~) {
  IterVisitorBase::visit_Expr_Hole(env, loc~, kind~)
}

///|
impl IterVisitor with visit_Expr_Return(env, return_value~, loc~) {
  IterVisitorBase::visit_Expr_Return(env, return_value~, loc~)
}

///|
impl IterVisitor with visit_Expr_Raise(env, err_value~, loc~) {
  IterVisitorBase::visit_Expr_Raise(env, err_value~, loc~)
}

///|
impl IterVisitor with visit_Expr_Unit(env, loc~, faked~) {
  IterVisitorBase::visit_Expr_Unit(env, loc~, faked~)
}

///|
impl IterVisitor with visit_Expr_Break(env, arg~, label~, loc~) {
  IterVisitorBase::visit_Expr_Break(env, arg~, label~, loc~)
}

///|
impl IterVisitor with visit_Expr_Continue(env, args~, label~, loc~) {
  IterVisitorBase::visit_Expr_Continue(env, args~, label~, loc~)
}

///|
impl IterVisitor with visit_Expr_Loop(env, arg~, body~, label~, loop_loc~, loc~) {
  IterVisitorBase::visit_Expr_Loop(env, arg~, body~, label~, loop_loc~, loc~)
}

///|
impl IterVisitor with visit_Expr_For(
  env,
  binders~,
  condition~,
  continue_block~,
  body~,
  for_else~,
  label~,
  loc~,
  where_clause~,
) {
  IterVisitorBase::visit_Expr_For(
    env,
    binders~,
    condition~,
    continue_block~,
    body~,
    for_else~,
    label~,
    loc~,
    where_clause~,
  )
}

///|
impl IterVisitor with visit_Expr_ForEach(
  env,
  binders~,
  expr~,
  body~,
  else_block~,
  label~,
  loc~,
) {
  IterVisitorBase::visit_Expr_ForEach(
    env,
    binders~,
    expr~,
    body~,
    else_block~,
    label~,
    loc~,
  )
}

///|
impl IterVisitor with visit_Expr_Try(
  env,
  body~,
  catch_~,
  catch_all~,
  try_else~,
  has_try~,
  try_loc~,
  catch_loc~,
  else_loc~,
  loc~,
) {
  IterVisitorBase::visit_Expr_Try(
    env,
    body~,
    catch_~,
    catch_all~,
    try_else~,
    has_try~,
    try_loc~,
    catch_loc~,
    else_loc~,
    loc~,
  )
}

///|
impl IterVisitor with visit_Expr_TryOperator(env, body~, kind~, try_loc~, loc~) {
  IterVisitorBase::visit_Expr_TryOperator(env, body~, kind~, try_loc~, loc~)
}

///|
impl IterVisitor with visit_Expr_Map(env, elems~, loc~) {
  IterVisitorBase::visit_Expr_Map(env, elems~, loc~)
}

///|
impl IterVisitor with visit_Expr_Group(env, expr~, group~, loc~) {
  IterVisitorBase::visit_Expr_Group(env, expr~, group~, loc~)
}

///|
impl IterVisitor with visit_Expr_StaticAssert(env, asserts~, body~) {
  IterVisitorBase::visit_Expr_StaticAssert(env, asserts~, body~)
}

///|
impl IterVisitor with visit_Visibility(env, visibility) {
  IterVisitorBase::visit_Visibility(env, visibility)
}

///|
impl IterVisitor with visit_Visibility_Pub(env, attr~, loc~) {
  IterVisitorBase::visit_Visibility_Pub(env, attr~, loc~)
}

///|
impl IterVisitor with visit_Visibility_Priv(env, loc~) {
  IterVisitorBase::visit_Visibility_Priv(env, loc~)
}

///|
impl IterVisitor with visit_Constant(env, constant) {
  IterVisitorBase::visit_Constant(env, constant)
}

///|
impl IterVisitor with visit_Label(env, label) {
  IterVisitorBase::visit_Label(env, label)
}

///|
impl IterVisitor with visit_ConstrName(env, constr_name) {
  IterVisitorBase::visit_ConstrName(env, constr_name)
}

///|
impl IterVisitor with visit_LongIdent(env, long_ident) {
  IterVisitorBase::visit_LongIdent(env, long_ident)
}

///|
impl IterVisitor with visit_LongIdent_Ident(env, name~) {
  IterVisitorBase::visit_LongIdent_Ident(env, name~)
}

///|
impl IterVisitor with visit_LongIdent_Dot(env, pkg~, id~) {
  IterVisitorBase::visit_LongIdent_Dot(env, pkg~, id~)
}

///|
impl IterVisitor with visit_TypeName(env, type_name) {
  IterVisitorBase::visit_TypeName(env, type_name)
}

///|
impl IterVisitor with visit_ConstrId(env, constr_id) {
  IterVisitorBase::visit_ConstrId(env, constr_id)
}

///|
impl IterVisitor with visit_Type(env, type_) {
  IterVisitorBase::visit_Type(env, type_)
}

///|
impl IterVisitor with visit_Type_Any(env, loc~) {
  IterVisitorBase::visit_Type_Any(env, loc~)
}

///|
impl IterVisitor with visit_Type_Arrow(env, args~, res~, err~, is_async~, loc~) {
  IterVisitorBase::visit_Type_Arrow(env, args~, res~, err~, is_async~, loc~)
}

///|
impl IterVisitor with visit_Type_Tuple(env, tys~, loc~) {
  IterVisitorBase::visit_Type_Tuple(env, tys~, loc~)
}

///|
impl IterVisitor with visit_Type_Name(env, constr_id~, tys~, loc~) {
  IterVisitorBase::visit_Type_Name(env, constr_id~, tys~, loc~)
}

///|
impl IterVisitor with visit_Type_Option(env, ty~, loc~, question_loc~) {
  IterVisitorBase::visit_Type_Option(env, ty~, loc~, question_loc~)
}

///|
impl IterVisitor with visit_Type_Object(env, constr_id) {
  IterVisitorBase::visit_Type_Object(env, constr_id)
}

///|
impl IterVisitor with visit_ErrorType(env, error_type) {
  IterVisitorBase::visit_ErrorType(env, error_type)
}

///|
impl IterVisitor with visit_ErrorType_ErrorType(env, ty~) {
  IterVisitorBase::visit_ErrorType_ErrorType(env, ty~)
}

///|
impl IterVisitor with visit_ErrorType_DefaultErrorType(env, loc~) {
  IterVisitorBase::visit_ErrorType_DefaultErrorType(env, loc~)
}

///|
impl IterVisitor with visit_ErrorType_Noraise(env, loc~) {
  IterVisitorBase::visit_ErrorType_Noraise(env, loc~)
}

///|
impl IterVisitor with visit_ErrorType_MaybeError(env, ty~) {
  IterVisitorBase::visit_ErrorType_MaybeError(env, ty~)
}

///|
impl IterVisitor with visit_ConstrParam(env, constr_param) {
  IterVisitorBase::visit_ConstrParam(env, constr_param)
}

///|
impl IterVisitor with visit_ConstrDecl(env, constr_decl) {
  IterVisitorBase::visit_ConstrDecl(env, constr_decl)
}

///|
impl IterVisitor with visit_ExceptionDecl(env, exception_decl) {
  IterVisitorBase::visit_ExceptionDecl(env, exception_decl)
}

///|
impl IterVisitor with visit_ExceptionDecl_SinglePayload(env, type_) {
  IterVisitorBase::visit_ExceptionDecl_SinglePayload(env, type_)
}

///|
impl IterVisitor with visit_ExceptionDecl_EnumPayload(env, constr_decls) {
  IterVisitorBase::visit_ExceptionDecl_EnumPayload(env, constr_decls)
}

///|
impl IterVisitor with visit_FieldName(env, field_name) {
  IterVisitorBase::visit_FieldName(env, field_name)
}

///|
impl IterVisitor with visit_FieldDecl(env, field_decl) {
  IterVisitorBase::visit_FieldDecl(env, field_decl)
}

///|
impl IterVisitor with visit_TypeDesc(env, type_desc) {
  IterVisitorBase::visit_TypeDesc(env, type_desc)
}

///|
impl IterVisitor with visit_TypeDesc_Newtype(env, type_) {
  IterVisitorBase::visit_TypeDesc_Newtype(env, type_)
}

///|
impl IterVisitor with visit_TypeDesc_Error(env, exception_decl) {
  IterVisitorBase::visit_TypeDesc_Error(env, exception_decl)
}

///|
impl IterVisitor with visit_TypeDesc_Variant(env, constr_decls) {
  IterVisitorBase::visit_TypeDesc_Variant(env, constr_decls)
}

///|
impl IterVisitor with visit_TypeDesc_Record(env, field_decls) {
  IterVisitorBase::visit_TypeDesc_Record(env, field_decls)
}

///|
impl IterVisitor with visit_TypeDesc_TupleStruct(env, types) {
  IterVisitorBase::visit_TypeDesc_TupleStruct(env, types)
}

///|
impl IterVisitor with visit_TypeDesc_Alias(env, type_) {
  IterVisitorBase::visit_TypeDesc_Alias(env, type_)
}

///|
impl IterVisitor with visit_Hole(env, hole) {
  IterVisitorBase::visit_Hole(env, hole)
}

///|
impl IterVisitor with visit_ArgumentKind(env, argument_kind) {
  IterVisitorBase::visit_ArgumentKind(env, argument_kind)
}

///|
impl IterVisitor with visit_ArgumentKind_Labelled(env, label) {
  IterVisitorBase::visit_ArgumentKind_Labelled(env, label)
}

///|
impl IterVisitor with visit_ArgumentKind_LabelledPun(env, label) {
  IterVisitorBase::visit_ArgumentKind_LabelledPun(env, label)
}

///|
impl IterVisitor with visit_ArgumentKind_LabelledOption(
  env,
  label~,
  question_loc~,
) {
  IterVisitorBase::visit_ArgumentKind_LabelledOption(env, label~, question_loc~)
}

///|
impl IterVisitor with visit_ArgumentKind_LabelledOptionPun(
  env,
  label~,
  question_loc~,
) {
  IterVisitorBase::visit_ArgumentKind_LabelledOptionPun(
    env,
    label~,
    question_loc~,
  )
}

///|
impl IterVisitor with visit_FnKind(env, fn_kind) {
  IterVisitorBase::visit_FnKind(env, fn_kind)
}

///|
impl IterVisitor with visit_Group(env, group) {
  IterVisitorBase::visit_Group(env, group)
}

///|
impl IterVisitor with visit_TrailingMark(env, trailing_mark) {
  IterVisitorBase::visit_TrailingMark(env, trailing_mark)
}

///|
impl IterVisitor with visit_ApplyAttr(env, apply_attr) {
  IterVisitorBase::visit_ApplyAttr(env, apply_attr)
}

///|
impl IterVisitor with visit_TypeVarConstraint(env, type_var_constraint) {
  IterVisitorBase::visit_TypeVarConstraint(env, type_var_constraint)
}

///|
impl IterVisitor with visit_TypeVarBinder(env, type_var_binder) {
  IterVisitorBase::visit_TypeVarBinder(env, type_var_binder)
}

///|
impl IterVisitor with visit_TypeDeclBinder(env, type_decl_binder) {
  IterVisitorBase::visit_TypeDeclBinder(env, type_decl_binder)
}

///|
impl IterVisitor with visit_Binder(env, binder) {
  IterVisitorBase::visit_Binder(env, binder)
}

///|
impl IterVisitor with visit_Var(env, var_) {
  IterVisitorBase::visit_Var(env, var_)
}

///|
impl IterVisitor with visit_ConstructorExtraInfo(env, constructor_extra_info) {
  IterVisitorBase::visit_ConstructorExtraInfo(env, constructor_extra_info)
}

///|
impl IterVisitor with visit_ConstructorExtraInfo_TypeName(env, type_name) {
  IterVisitorBase::visit_ConstructorExtraInfo_TypeName(env, type_name)
}

///|
impl IterVisitor with visit_ConstructorExtraInfo_Package(env, package_) {
  IterVisitorBase::visit_ConstructorExtraInfo_Package(env, package_)
}

///|
impl IterVisitor with visit_Constructor(env, constructor_) {
  IterVisitorBase::visit_Constructor(env, constructor_)
}

///|
impl IterVisitor with visit_Accessor(env, accessor) {
  IterVisitorBase::visit_Accessor(env, accessor)
}

///|
impl IterVisitor with visit_Accessor_Label(env, label) {
  IterVisitorBase::visit_Accessor_Label(env, label)
}

///|
impl IterVisitor with visit_Accessor_Index(env, tuple_index~, loc~) {
  IterVisitorBase::visit_Accessor_Index(env, tuple_index~, loc~)
}

///|
impl IterVisitor with visit_Accessor_Newtype(env, loc~) {
  IterVisitorBase::visit_Accessor_Newtype(env, loc~)
}

///|
impl IterVisitor with visit_AliasTarget(env, alias_target) {
  IterVisitorBase::visit_AliasTarget(env, alias_target)
}

///|
impl IterVisitor with visit_Argument(env, argument) {
  IterVisitorBase::visit_Argument(env, argument)
}

///|
impl IterVisitor with visit_Parameter(env, parameter) {
  IterVisitorBase::visit_Parameter(env, parameter)
}

///|
impl IterVisitor with visit_Parameter_DiscardPositional(env, ty~, loc~) {
  IterVisitorBase::visit_Parameter_DiscardPositional(env, ty~, loc~)
}

///|
impl IterVisitor with visit_Parameter_Positional(env, binder~, ty~) {
  IterVisitorBase::visit_Parameter_Positional(env, binder~, ty~)
}

///|
impl IterVisitor with visit_Parameter_Labelled(env, binder~, ty~) {
  IterVisitorBase::visit_Parameter_Labelled(env, binder~, ty~)
}

///|
impl IterVisitor with visit_Parameter_Optional(env, binder~, default~, ty~) {
  IterVisitorBase::visit_Parameter_Optional(env, binder~, default~, ty~)
}

///|
impl IterVisitor with visit_Parameter_QuestionOptional(env, binder~, ty~) {
  IterVisitorBase::visit_Parameter_QuestionOptional(env, binder~, ty~)
}

///|
impl IterVisitor with visit_Case(env, case) {
  IterVisitorBase::visit_Case(env, case)
}

///|
impl IterVisitor with visit_SpreadableElem(env, spreadable_elem) {
  IterVisitorBase::visit_SpreadableElem(env, spreadable_elem)
}

///|
impl IterVisitor with visit_SpreadableElem_Regular(env, expr) {
  IterVisitorBase::visit_SpreadableElem_Regular(env, expr)
}

///|
impl IterVisitor with visit_SpreadableElem_Spread(env, expr~, loc~) {
  IterVisitorBase::visit_SpreadableElem_Spread(env, expr~, loc~)
}

///|
impl IterVisitor with visit_MapExprElem(env, map_pat_elem) {
  IterVisitorBase::visit_MapExprElem(env, map_pat_elem)
}

///|
impl IterVisitor with visit_StaticAssertion(env, static_assertion) {
  IterVisitorBase::visit_StaticAssertion(env, static_assertion)
}

///|
impl IterVisitor with visit_Func(env, func) {
  IterVisitorBase::visit_Func(env, func)
}

///|
impl IterVisitor with visit_FieldDef(env, field_def) {
  IterVisitorBase::visit_FieldDef(env, field_def)
}

///|
impl IterVisitor with visit_InterpElem(env, interp_elem) {
  IterVisitorBase::visit_InterpElem(env, interp_elem)
}

///|
impl IterVisitor with visit_InterpElem_Literal(env, repr~, loc~) {
  IterVisitorBase::visit_InterpElem_Literal(env, repr~, loc~)
}

///|
impl IterVisitor with visit_InterpElem_Expr(env, expr~, loc~) {
  IterVisitorBase::visit_InterpElem_Expr(env, expr~, loc~)
}

///|
impl IterVisitor with visit_InterpElem_Source(env, interp_source) {
  IterVisitorBase::visit_InterpElem_Source(env, interp_source)
}

///|
impl IterVisitor with visit_MultilineStringElem(env, multiline_string_elem) {
  IterVisitorBase::visit_MultilineStringElem(env, multiline_string_elem)
}

///|
impl IterVisitor with visit_MultilineStringElem_String(env, string_val) {
  IterVisitorBase::visit_MultilineStringElem_String(env, string_val)
}

///|
impl IterVisitor with visit_MultilineStringElem_Interp(env, interp_elems) {
  IterVisitorBase::visit_MultilineStringElem_Interp(env, interp_elems)
}

///|
impl IterVisitor with visit_TryOperatorKind(env, try_operator_kind) {
  IterVisitorBase::visit_TryOperatorKind(env, try_operator_kind)
}

///|
impl IterVisitor with visit_DotDotBinder(env, dot_dot_binder) {
  IterVisitorBase::visit_DotDotBinder(env, dot_dot_binder)
}

///|
impl IterVisitor with visit_DotDotBinder_BinderAs(env, binder) {
  IterVisitorBase::visit_DotDotBinder_BinderAs(env, binder)
}

///|
impl IterVisitor with visit_DotDotBinder_Binder(env, binder) {
  IterVisitorBase::visit_DotDotBinder_Binder(env, binder)
}

///|
impl IterVisitor with visit_ArrayPattern(env, array_pattern) {
  IterVisitorBase::visit_ArrayPattern(env, array_pattern)
}

///|
impl IterVisitor with visit_ArrayPattern_Pattern(env, pattern) {
  IterVisitorBase::visit_ArrayPattern_Pattern(env, pattern)
}

///|
impl IterVisitor with visit_ArrayPattern_StringSpread(env, str~, loc~) {
  IterVisitorBase::visit_ArrayPattern_StringSpread(env, str~, loc~)
}

///|
impl IterVisitor with visit_ArrayPattern_BytesSpread(env, bytes~, loc~) {
  IterVisitorBase::visit_ArrayPattern_BytesSpread(env, bytes~, loc~)
}

///|
impl IterVisitor with visit_ArrayPattern_ConstSpread(env, binder~, pkg~, loc~) {
  IterVisitorBase::visit_ArrayPattern_ConstSpread(env, binder~, pkg~, loc~)
}

///|
impl IterVisitor with visit_ArrayPatterns(env, array_patterns) {
  IterVisitorBase::visit_ArrayPatterns(env, array_patterns)
}

///|
impl IterVisitor with visit_ArrayPatterns_Closed(env, array_patterns) {
  IterVisitorBase::visit_ArrayPatterns_Closed(env, array_patterns)
}

///|
impl IterVisitor with visit_ArrayPatterns_Open(
  env,
  prefix_patterns,
  suffix_patterns,
  dot_dot_binder,
) {
  IterVisitorBase::visit_ArrayPatterns_Open(
    env, prefix_patterns, suffix_patterns, dot_dot_binder,
  )
}

///|
impl IterVisitor with visit_WhereClause(env, where_clause) {
  IterVisitorBase::visit_WhereClause(env, where_clause)
}

///|
impl IterVisitor with visit_FieldPat(env, field_pat) {
  IterVisitorBase::visit_FieldPat(env, field_pat)
}

///|
impl IterVisitor with visit_ConstrPatArg(env, constr_pat_arg) {
  IterVisitorBase::visit_ConstrPatArg(env, constr_pat_arg)
}

///|
impl IterVisitor with visit_MapPatElem(env, map_pat_elem) {
  IterVisitorBase::visit_MapPatElem(env, map_pat_elem)
}

///|
impl IterVisitor with visit_RangeKind(env, kind) {
  IterVisitorBase::visit_RangeKind(env, kind)
}

///|
impl IterVisitor with visit_Pattern(env, pattern) {
  IterVisitorBase::visit_Pattern(env, pattern)
}

///|
impl IterVisitor with visit_Pattern_Alias(env, pat~, alias_~, loc~) {
  IterVisitorBase::visit_Pattern_Alias(env, pat~, alias_~, loc~)
}

///|
impl IterVisitor with visit_Pattern_Any(env, loc~) {
  IterVisitorBase::visit_Pattern_Any(env, loc~)
}

///|
impl IterVisitor with visit_Pattern_Array(env, pats~, loc~) {
  IterVisitorBase::visit_Pattern_Array(env, pats~, loc~)
}

///|
impl IterVisitor with visit_Pattern_Constant(env, c~, loc~) {
  IterVisitorBase::visit_Pattern_Constant(env, c~, loc~)
}

///|
impl IterVisitor with visit_Pattern_Constraint(env, pat~, ty~, loc~) {
  IterVisitorBase::visit_Pattern_Constraint(env, pat~, ty~, loc~)
}

///|
impl IterVisitor with visit_Pattern_Constr(env, constr~, args~, is_open~, loc~) {
  IterVisitorBase::visit_Pattern_Constr(env, constr~, args~, is_open~, loc~)
}

///|
impl IterVisitor with visit_Pattern_Or(env, pat1~, pat2~, loc~) {
  IterVisitorBase::visit_Pattern_Or(env, pat1~, pat2~, loc~)
}

///|
impl IterVisitor with visit_Pattern_Tuple(env, pats~, loc~) {
  IterVisitorBase::visit_Pattern_Tuple(env, pats~, loc~)
}

///|
impl IterVisitor with visit_Pattern_Var(env, binder) {
  IterVisitorBase::visit_Pattern_Var(env, binder)
}

///|
impl IterVisitor with visit_Pattern_Record(env, fields~, is_closed~, loc~) {
  IterVisitorBase::visit_Pattern_Record(env, fields~, is_closed~, loc~)
}

///|
impl IterVisitor with visit_Pattern_Map(env, elems~, is_closed~, loc~) {
  IterVisitorBase::visit_Pattern_Map(env, elems~, is_closed~, loc~)
}

///|
impl IterVisitor with visit_Pattern_Range(env, lhs~, rhs~, kind~, loc~) {
  IterVisitorBase::visit_Pattern_Range(env, lhs~, rhs~, kind~, loc~)
}

///|
impl IterVisitor with visit_Pattern_SpecialConstr(env, binder~, args~, loc~) {
  IterVisitorBase::visit_Pattern_SpecialConstr(env, binder~, args~, loc~)
}

///|
impl IterVisitor with visit_LocalTypeDecl(env, local_type_decl) {
  IterVisitorBase::visit_LocalTypeDecl(env, local_type_decl)
}

///|
impl IterVisitor with visit_DerivingDirective(env, deriving_directive) {
  IterVisitorBase::visit_DerivingDirective(env, deriving_directive)
}

///|
impl IterVisitor with visit_TypeDecl(env, type_decl) {
  IterVisitorBase::visit_TypeDecl(env, type_decl)
}

///|
impl IterVisitor with visit_FuncStubs(env, func_stubs) {
  IterVisitorBase::visit_FuncStubs(env, func_stubs)
}

///|
impl IterVisitor with visit_FuncStubs_Import(
  env,
  module_name~,
  func_name~,
  language~,
) {
  IterVisitorBase::visit_FuncStubs_Import(
    env,
    module_name~,
    func_name~,
    language~,
  )
}

///|
impl IterVisitor with visit_FuncStubs_Embedded(env, language~, code~) {
  IterVisitorBase::visit_FuncStubs_Embedded(env, language~, code~)
}

///|
impl IterVisitor with visit_EmbeddedCode(env, embedded_code) {
  IterVisitorBase::visit_EmbeddedCode(env, embedded_code)
}

///|
impl IterVisitor with visit_EmbeddedCode_CodeString(env, string_literal) {
  IterVisitorBase::visit_EmbeddedCode_CodeString(env, string_literal)
}

///|
impl IterVisitor with visit_EmbeddedCode_CodeMultilineString(env, strings) {
  IterVisitorBase::visit_EmbeddedCode_CodeMultilineString(env, strings)
}

///|
impl IterVisitor with visit_DeclBody(env, decl_body) {
  IterVisitorBase::visit_DeclBody(env, decl_body)
}

///|
impl IterVisitor with visit_DeclBody_DeclBody(env, local_types~, expr~) {
  IterVisitorBase::visit_DeclBody_DeclBody(env, local_types~, expr~)
}

///|
impl IterVisitor with visit_DeclBody_DeclStubs(env, func_stubs) {
  IterVisitorBase::visit_DeclBody_DeclStubs(env, func_stubs)
}

///|
impl IterVisitor with visit_FunDecl(env, fun_decl) {
  IterVisitorBase::visit_FunDecl(env, fun_decl)
}

///|
impl IterVisitor with visit_TraitMethodDecl(env, trait_method_decl) {
  IterVisitorBase::visit_TraitMethodDecl(env, trait_method_decl)
}

///|
impl IterVisitor with visit_TraitDecl(env, trait_decl) {
  IterVisitorBase::visit_TraitDecl(env, trait_decl)
}

///|
impl IterVisitor with visit_Expr_IsLexMatch(
  env,
  expr~ : Expr,
  strategy~,
  pat~,
  pat_loc~,
  loc~,
) {
  IterVisitorBase::visit_Expr_IsLexMatch(
    env,
    expr~,
    strategy~,
    pat~,
    pat_loc~,
    loc~,
  )
}

///|
impl IterVisitor with visit_Expr_LexMatch(
  env,
  strategy~,
  expr~,
  match_loc~,
  cases~,
  loc~,
) {
  IterVisitorBase::visit_Expr_LexMatch(
    env,
    strategy~,
    expr~,
    match_loc~,
    cases~,
    loc~,
  )
}

///|
impl IterVisitor with visit_LexCase(env, lex_case) {
  IterVisitorBase::visit_LexCase(env, lex_case)
}

///|
impl IterVisitor with visit_LexTopPattern(env, lex_top_pattern) {
  IterVisitorBase::visit_LexTopPattern(env, lex_top_pattern)
}

///|
impl IterVisitor with visit_LexTopPattern_Pattern(env, lex_pattern) {
  IterVisitorBase::visit_LexTopPattern_Pattern(env, lex_pattern)
}

///|
impl IterVisitor with visit_LexTopPattern_Binder(env, binder) {
  IterVisitorBase::visit_LexTopPattern_Binder(env, binder)
}

///|
impl IterVisitor with visit_LexTopPattern_Wildcard(env, loc~) {
  IterVisitorBase::visit_LexTopPattern_Wildcard(env, loc~)
}

///|
impl IterVisitor with visit_LexPattern(env, lex_pattern) {
  IterVisitorBase::visit_LexPattern(env, lex_pattern)
}

///|
impl IterVisitor with visit_LexPattern_Regex(env, lit~, loc~) {
  IterVisitorBase::visit_LexPattern_Regex(env, lit~, loc~)
}

///|
impl IterVisitor with visit_LexPattern_RegexInterp(env, elems~, loc~) {
  IterVisitorBase::visit_LexPattern_RegexInterp(env, elems~, loc~)
}

///|
impl IterVisitor with visit_LexPattern_Alias(env, pat~, binder~, loc~) {
  IterVisitorBase::visit_LexPattern_Alias(env, pat~, binder~, loc~)
}

///|
impl IterVisitor with visit_LexPattern_Sequence(env, pats~, loc~) {
  IterVisitorBase::visit_LexPattern_Sequence(env, pats~, loc~)
}

///|
pub struct IterVisitorBase[T](T)

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Impl(
  env,
  impl_,
) {
  match impl_ {
    TopView(
      quantifiers~,
      source_ty~,
      view_type_name~,
      view_type_loc~,
      view_constrs~,
      parameters~,
      view_func_name~,
      params_loc~,
      body~,
      vis~,
      attrs~,
      loc~,
      doc~
    ) =>
      env.0.visit_Impl_TopView(
        quantifiers~,
        source_ty~,
        view_type_name~,
        view_type_loc~,
        view_constrs~,
        view_func_name~,
        parameters~,
        params_loc~,
        body~,
        vis~,
        loc~,
        attrs~,
        doc~,
      )
    TopImpl(
      self_ty~,
      trait_~,
      method_name~,
      has_error~,
      quantifiers~,
      params~,
      ret_ty~,
      err_ty~,
      body~,
      vis~,
      loc~,
      attrs~,
      doc~
    ) =>
      env.0.visit_Impl_TopImpl(
        self_ty~,
        trait_~,
        method_name~,
        has_error~,
        quantifiers~,
        params~,
        ret_ty~,
        err_ty~,
        body~,
        vis~,
        loc~,
        attrs~,
        doc~,
      )
    TopBatchTraitAlias(pkg~, targets~, vis~, loc~, attrs~, is_list~, doc~) =>
      env.0.visit_Impl_TopBatchTraitAlias(
        pkg~,
        targets~,
        vis~,
        loc~,
        attrs~,
        is_list~,
        doc~,
      )
    TopBatchTypeAlias(pkg~, targets~, vis~, loc~, attrs~, is_list~, doc~) =>
      env.0.visit_Impl_TopBatchTypeAlias(
        pkg~,
        targets~,
        vis~,
        loc~,
        attrs~,
        is_list~,
        doc~,
      )
    TopTrait(trait_decl) => env.0.visit_Impl_TopTrait(trait_decl)
    TopLetDef(
      binder~,
      ty~,
      expr~,
      vis~,
      is_constant~,
      loc~,
      attrs~,
      doc~,
      is_declare~
    ) =>
      env.0.visit_Impl_TopLetDef(
        binder~,
        ty~,
        expr~,
        vis~,
        is_constant~,
        loc~,
        attrs~,
        doc~,
        is_declare~,
      )
    TopFuncAlias(pkg~, type_name~, targets~, vis~, attrs~, is_list~, doc~, loc~) =>
      env.0.visit_Impl_TopFuncAlias(
        pkg~,
        type_name~,
        targets~,
        vis~,
        attrs~,
        is_list~,
        doc~,
        loc~,
      )
    TopFuncDef(fun_decl~, decl_body~, loc~) =>
      env.0.visit_Impl_TopFuncDef(fun_decl~, decl_body~, loc~)
    TopTypeDef(type_decl) => env.0.visit_Impl_TopTypeDef(type_decl)
    TopTest(expr~, name~, params~, local_types~, is_async~, loc~, attrs~, doc~) =>
      env.0.visit_Impl_TopTest(
        expr~,
        name~,
        params~,
        local_types~,
        is_async~,
        loc~,
        attrs~,
        doc~,
      )
    TopExpr(expr~, is_main~, local_types~, is_async~, loc~) =>
      env.0.visit_Impl_TopExpr(expr~, is_main~, local_types~, is_async~, loc~)
    TopImplRelation(
      self_ty~,
      trait_~,
      quantifiers~,
      vis~,
      attrs~,
      loc~,
      doc~,
      is_declare~
    ) =>
      env.0.visit_Impl_TopImplRelation(
        self_ty~,
        trait_~,
        quantifiers~,
        vis~,
        attrs~,
        loc~,
        doc~,
        is_declare~,
      )
    TopUsing(pkg~, names~, vis~, attrs~, loc~, doc~) =>
      env.0.visit_Impl_TopUsing(pkg~, names~, vis~, attrs~, loc~, doc~)
  }
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Visibility(
  env,
  visibility,
) {
  match visibility {
    Default => ()
    Pub(attr~, loc~) => env.0.visit_Visibility_Pub(attr~, loc~)
    Priv(loc~) => env.0.visit_Visibility_Priv(loc~)
  }
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Visibility_Pub(
  _,
  attr~,
  loc~,
) {
  ignore(attr)
  ignore(loc)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Visibility_Priv(
  _,
  loc~,
) {
  ignore(loc)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Constant(
  _,
  _,
) {
  ()
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Label(
  _,
  _,
) {
  ()
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_ConstrName(
  _,
  _,
) {
  ()
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_LongIdent(
  env,
  long_ident,
) {
  match long_ident {
    Ident(name~) => env.0.visit_LongIdent_Ident(name~)
    Dot(pkg~, id~) => env.0.visit_LongIdent_Dot(pkg~, id~)
  }
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_LongIdent_Ident(
  _,
  name~,
) {
  ignore(name)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_LongIdent_Dot(
  _,
  pkg~,
  id~,
) {
  ignore(pkg)
  ignore(id)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_TypeName(
  env,
  type_name,
) {
  env.0.visit_LongIdent(type_name.name)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_ConstrId(
  env,
  constr_id,
) {
  env.0.visit_LongIdent(constr_id.id)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Type(
  env,
  type_,
) {
  match type_ {
    Any(loc~) => env.0.visit_Type_Any(loc~)
    Arrow(args~, res~, err~, is_async~, loc~) =>
      env.0.visit_Type_Arrow(args~, res~, err~, is_async~, loc~)
    Tuple(tys~, loc~) => env.0.visit_Type_Tuple(tys~, loc~)
    Name(constr_id~, tys~, loc~) =>
      env.0.visit_Type_Name(constr_id~, tys~, loc~)
    Option(ty~, loc~, question_loc~) =>
      env.0.visit_Type_Option(ty~, loc~, question_loc~)
    Object(constr_id) => env.0.visit_Type_Object(constr_id)
  }
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Type_Any(
  _,
  loc~,
) {
  ignore(loc)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Type_Arrow(
  env,
  args~,
  res~,
  err~,
  is_async~,
  loc~,
) {
  ignore(is_async)
  ignore(loc)
  args.each(x => env.0.visit_Type(x))
  env.0.visit_Type(res)
  env.0.visit_ErrorType(err)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Type_Tuple(
  env,
  tys~,
  loc~,
) {
  ignore(loc)
  tys.each(x => env.0.visit_Type(x))
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Type_Name(
  env,
  constr_id~,
  tys~,
  loc~,
) {
  ignore(loc)
  env.0.visit_ConstrId(constr_id)
  tys.each(x => env.0.visit_Type(x))
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Type_Option(
  env,
  ty~,
  loc~,
  question_loc~,
) {
  ignore(loc)
  ignore(question_loc)
  env.0.visit_Type(ty)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Type_Object(
  env,
  constr_id,
) {
  env.0.visit_ConstrId(constr_id)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_ErrorType(
  env,
  error_type,
) {
  match error_type {
    ErrorType(ty~) => env.0.visit_ErrorType_ErrorType(ty~)
    DefaultErrorType(loc~) => env.0.visit_ErrorType_DefaultErrorType(loc~)
    NoErrorType => ()
    Noraise(loc~) => env.0.visit_ErrorType_Noraise(loc~)
    MaybeError(ty~) => env.0.visit_ErrorType_MaybeError(ty~)
  }
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_ErrorType_ErrorType(
  env,
  ty~,
) {
  env.0.visit_Type(ty)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_ErrorType_DefaultErrorType(
  _,
  loc~,
) {
  ignore(loc)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_ErrorType_Noraise(
  _,
  loc~,
) {
  ignore(loc)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_ErrorType_MaybeError(
  env,
  ty~,
) {
  env.0.visit_Type(ty)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_ConstrParam(
  env,
  constr_param,
) {
  env.0.visit_Type(constr_param.ty)
  constr_param.label.each(x => env.0.visit_Label(x))
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_ConstrDecl(
  env,
  constr_decl,
) {
  env.0.visit_ConstrName(constr_decl.name)
  constr_decl.args.each(args => args.each(x => env.0.visit_ConstrParam(x)))
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_ExceptionDecl(
  env,
  exception_decl,
) {
  match exception_decl {
    NoPayload => ()
    SinglePayload(type_) => env.0.visit_ExceptionDecl_SinglePayload(type_)
    EnumPayload(constr_decls) =>
      env.0.visit_ExceptionDecl_EnumPayload(constr_decls)
  }
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_ExceptionDecl_SinglePayload(
  env,
  type_,
) {
  env.0.visit_Type(type_)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_ExceptionDecl_EnumPayload(
  env,
  constr_decls,
) {
  constr_decls.each(x => env.0.visit_ConstrDecl(x))
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_FieldName(
  _,
  _,
) {
  ()
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_FieldDecl(
  env,
  field_decl,
) {
  env.0.visit_FieldName(field_decl.name)
  env.0.visit_Type(field_decl.ty)
  env.0.visit_Visibility(field_decl.vis)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_TypeDesc(
  env,
  type_desc,
) {
  match type_desc {
    Abstract => ()
    Extern => ()
    Newtype(type_) => env.0.visit_TypeDesc_Newtype(type_)
    Error(exception_decl) => env.0.visit_TypeDesc_Error(exception_decl)
    Variant(constr_decls) => env.0.visit_TypeDesc_Variant(constr_decls)
    Record(field_decls) => env.0.visit_TypeDesc_Record(field_decls)
    TupleStruct(types) => env.0.visit_TypeDesc_TupleStruct(types)
    Alias(type_) => env.0.visit_TypeDesc_Alias(type_)
  }
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_TypeDesc_Newtype(
  env,
  type_,
) {
  env.0.visit_Type(type_)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_TypeDesc_Error(
  env,
  exception_decl,
) {
  env.0.visit_ExceptionDecl(exception_decl)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_TypeDesc_Variant(
  env,
  constr_decls,
) {
  constr_decls.each(x => env.0.visit_ConstrDecl(x))
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_TypeDesc_Record(
  env,
  field_decls,
) {
  field_decls.each(x => env.0.visit_FieldDecl(x))
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_TypeDesc_TupleStruct(
  env,
  types,
) {
  types.each(x => env.0.visit_Type(x))
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_TypeDesc_Alias(
  env,
  type_,
) {
  env.0.visit_Type(type_)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Hole(
  _,
  hole,
) {
  ignore(hole)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_ArgumentKind(
  env,
  argument_kind,
) {
  match argument_kind {
    Positional => ()
    Labelled(label) => env.0.visit_ArgumentKind_Labelled(label)
    LabelledPun(label) => env.0.visit_ArgumentKind_LabelledPun(label)
    LabelledOption(label~, question_loc~) =>
      env.0.visit_ArgumentKind_LabelledOption(label~, question_loc~)
    LabelledOptionPun(label~, question_loc~) =>
      env.0.visit_ArgumentKind_LabelledOptionPun(label~, question_loc~)
  }
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_ArgumentKind_Labelled(
  _,
  label,
) {
  ignore(label)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_ArgumentKind_LabelledPun(
  _,
  label,
) {
  ignore(label)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_ArgumentKind_LabelledOption(
  _,
  label~,
  question_loc~,
) {
  ignore(label)
  ignore(question_loc)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_ArgumentKind_LabelledOptionPun(
  _,
  label~,
  question_loc~,
) {
  ignore(label)
  ignore(question_loc)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_FnKind(
  _,
  _,
) {
  ()
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Group(
  _,
  _,
) {
  ()
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_TrailingMark(
  _,
  _,
) {
  ()
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_ApplyAttr(
  _,
  _,
) {
  ()
}

// Continue with remaining IterVisitorBase implementations that are missing

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_TypeVarConstraint(
  env,
  type_var_constraint,
) {
  env.0.visit_LongIdent(type_var_constraint.trait_)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_TypeVarBinder(
  env,
  type_var_binder,
) {
  type_var_binder.constraints.each(x => env.0.visit_TypeVarConstraint(x))
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_TypeDeclBinder(
  _,
  _,
) {
  ()
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Binder(
  _,
  _,
) {
  ()
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Var(
  env,
  var_,
) {
  env.0.visit_LongIdent(var_.name)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_ConstructorExtraInfo(
  env,
  constructor_extra_info,
) {
  match constructor_extra_info {
    TypeName(type_name) => env.0.visit_ConstructorExtraInfo_TypeName(type_name)
    Package(package_) => env.0.visit_ConstructorExtraInfo_Package(package_)
    NoExtraInfo => ()
  }
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_ConstructorExtraInfo_TypeName(
  env,
  type_name,
) {
  env.0.visit_TypeName(type_name)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_ConstructorExtraInfo_Package(
  _,
  _,
) {
  ()
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Constructor(
  env,
  constructor_,
) {
  env.0.visit_ConstrName(constructor_.name)
  env.0.visit_ConstructorExtraInfo(constructor_.extra_info)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Accessor(
  env,
  accessor,
) {
  match accessor {
    Label(label) => env.0.visit_Accessor_Label(label)
    Index(tuple_index~, loc~) => env.0.visit_Accessor_Index(tuple_index~, loc~)
    Newtype(loc~) => env.0.visit_Accessor_Newtype(loc~)
  }
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Accessor_Label(
  env,
  label,
) {
  env.0.visit_Label(label)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Accessor_Index(
  _,
  tuple_index~,
  loc~,
) {
  ignore(tuple_index)
  ignore(loc)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Accessor_Newtype(
  _,
  loc~,
) {
  ignore(loc)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_AliasTarget(
  env,
  alias_target,
) {
  env.0.visit_Binder(alias_target.binder)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Argument(
  env,
  argument,
) {
  env.0.visit_Expr(argument.value)
  env.0.visit_ArgumentKind(argument.kind)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Parameter(
  env,
  parameter,
) {
  match parameter {
    DiscardPositional(ty~, loc~) =>
      env.0.visit_Parameter_DiscardPositional(ty~, loc~)
    Positional(binder~, ty~) => env.0.visit_Parameter_Positional(binder~, ty~)
    Labelled(binder~, ty~) => env.0.visit_Parameter_Labelled(binder~, ty~)
    Optional(binder~, default~, ty~) =>
      env.0.visit_Parameter_Optional(binder~, default~, ty~)
    QuestionOptional(binder~, ty~) =>
      env.0.visit_Parameter_QuestionOptional(binder~, ty~)
  }
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Parameter_DiscardPositional(
  env,
  ty~,
  loc~,
) {
  ignore(loc)
  ty.each(x => env.0.visit_Type(x))
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Parameter_Positional(
  env,
  binder~,
  ty~,
) {
  env.0.visit_Binder(binder)
  ty.each(x => env.0.visit_Type(x))
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Parameter_Labelled(
  env,
  binder~,
  ty~,
) {
  ignore(ty)
  env.0.visit_Binder(binder)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Parameter_Optional(
  env,
  binder~,
  default~,
  ty~,
) {
  env.0.visit_Binder(binder)
  env.0.visit_Expr(default)
  ty.each(x => env.0.visit_Type(x))
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Parameter_QuestionOptional(
  env,
  binder~,
  ty~,
) {
  env.0.visit_Binder(binder)
  ty.each(x => env.0.visit_Type(x))
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Case(
  env,
  case,
) {
  env.0.visit_Pattern(case.pattern)
  case.guard_.each(x => env.0.visit_Expr(x))
  env.0.visit_Expr(case.body)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_SpreadableElem(
  env,
  spreadable_elem,
) {
  match spreadable_elem {
    Regular(expr) => env.0.visit_SpreadableElem_Regular(expr)
    Spread(expr~, loc~) => env.0.visit_SpreadableElem_Spread(expr~, loc~)
  }
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_SpreadableElem_Regular(
  env,
  expr,
) {
  env.0.visit_Expr(expr)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_SpreadableElem_Spread(
  env,
  expr~,
  loc~,
) {
  ignore(loc)
  env.0.visit_Expr(expr)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_MapExprElem(
  env,
  map_expr_elem,
) {
  env.0.visit_Constant(map_expr_elem.key)
  env.0.visit_Expr(map_expr_elem.expr)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_StaticAssertion(
  env,
  static_assertion,
) {
  env.0.visit_Type(static_assertion.ty)
  env.0.visit_LongIdent(static_assertion.trait_)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Func(
  env,
  func,
) {
  let {
    is_async,
    parameters,
    params_loc,
    body,
    return_type,
    error_type,
    kind,
    has_error,
    loc,
  } = func
  ignore(is_async)
  ignore(params_loc)
  ignore(loc)
  ignore(has_error)
  parameters.each(p => env.0.visit_Parameter(p))
  env.0.visit_Expr(body)
  return_type.each(t => env.0.visit_Type(t))
  env.0.visit_ErrorType(error_type)
  env.0.visit_FnKind(kind)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_FieldDef(
  env,
  field_def,
) {
  env.0.visit_Label(field_def.label)
  env.0.visit_Expr(field_def.expr)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_InterpElem(
  env,
  interp_elem,
) {
  match interp_elem {
    Literal(repr~, loc~) => env.0.visit_InterpElem_Literal(repr~, loc~)
    Expr(expr~, loc~) => env.0.visit_InterpElem_Expr(expr~, loc~)
    Source(interp_source) => env.0.visit_InterpElem_Source(interp_source)
  }
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_InterpElem_Literal(
  _,
  repr~,
  loc~,
) {
  ignore(repr)
  ignore(loc)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_InterpElem_Expr(
  env,
  expr~,
  loc~,
) {
  ignore(loc)
  env.0.visit_Expr(expr)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_InterpElem_Source(
  _,
  interp_source,
) {
  ignore(interp_source)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_MultilineStringElem(
  env,
  multiline_string_elem,
) {
  match multiline_string_elem {
    String(string_val) => env.0.visit_MultilineStringElem_String(string_val)
    Interp(interp_elems) => env.0.visit_MultilineStringElem_Interp(interp_elems)
  }
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_MultilineStringElem_String(
  _,
  string_val,
) {
  ignore(string_val)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_MultilineStringElem_Interp(
  _,
  interp_elems,
) {
  ignore(interp_elems)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_TryOperatorKind(
  _,
  try_operator_kind,
) {
  ignore(try_operator_kind)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_DotDotBinder(
  env,
  dot_dot_binder,
) {
  match dot_dot_binder {
    Underscore => ()
    NoBinder => ()
    BinderAs(binder) => env.0.visit_DotDotBinder_BinderAs(binder)
    Binder(binder) => env.0.visit_DotDotBinder_Binder(binder)
  }
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_DotDotBinder_BinderAs(
  env,
  binder,
) {
  env.0.visit_Binder(binder)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_DotDotBinder_Binder(
  env,
  binder,
) {
  env.0.visit_Binder(binder)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_ArrayPattern(
  env,
  array_pattern,
) {
  match array_pattern {
    Pattern(pattern) => env.0.visit_ArrayPattern_Pattern(pattern)
    StringSpread(str~, loc~) =>
      env.0.visit_ArrayPattern_StringSpread(str~, loc~)
    BytesSpread(bytes~, loc~) =>
      env.0.visit_ArrayPattern_BytesSpread(bytes~, loc~)
    ConstSpread(binder~, pkg~, loc~) =>
      env.0.visit_ArrayPattern_ConstSpread(binder~, pkg~, loc~)
  }
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_ArrayPattern_Pattern(
  env,
  pattern,
) {
  env.0.visit_Pattern(pattern)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_ArrayPattern_StringSpread(
  _,
  str~,
  loc~,
) {
  ignore((str, loc))
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_ArrayPattern_BytesSpread(
  _,
  bytes~,
  loc~,
) {
  ignore((bytes, loc))
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_ArrayPattern_ConstSpread(
  env,
  binder~,
  pkg~,
  loc~,
) {
  ignore(pkg)
  ignore(loc)
  env.0.visit_Binder(binder)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_ArrayPatterns(
  env,
  array_patterns,
) {
  match array_patterns {
    Closed(patterns) => env.0.visit_ArrayPatterns_Closed(patterns)
    Open(prefix_patterns, suffix_patterns, dot_dot_binder) =>
      env.0.visit_ArrayPatterns_Open(
        prefix_patterns, suffix_patterns, dot_dot_binder,
      )
  }
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_ArrayPatterns_Closed(
  env,
  array_patterns,
) {
  array_patterns.each(x => env.0.visit_ArrayPattern(x))
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_ArrayPatterns_Open(
  env,
  prefix_patterns,
  suffix_patterns,
  dot_dot_binder,
) {
  prefix_patterns.each(x => env.0.visit_ArrayPattern(x))
  suffix_patterns.each(x => env.0.visit_ArrayPattern(x))
  env.0.visit_DotDotBinder(dot_dot_binder)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_WhereClause(
  env,
  where_clause,
) {
  where_clause.fields.each(x => env.0.visit_FieldDef(x))
  ignore(where_clause.loc)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_FieldPat(
  env,
  field_pat,
) {
  env.0.visit_Label(field_pat.label)
  env.0.visit_Pattern(field_pat.pattern)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_ConstrPatArg(
  env,
  constr_pat_arg,
) {
  env.0.visit_Pattern(constr_pat_arg.pat)
  env.0.visit_ArgumentKind(constr_pat_arg.kind)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_MapPatElem(
  env,
  map_pat_elem,
) {
  env.0.visit_Constant(map_pat_elem.key)
  env.0.visit_Pattern(map_pat_elem.pat)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_RangeKind(
  env,
  kind,
) {
  ignore(env)
  ignore(kind)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Pattern(
  env,
  pattern,
) {
  match pattern {
    Alias(pat~, alias_~, loc~) => env.0.visit_Pattern_Alias(pat~, alias_~, loc~)
    Any(loc~) => env.0.visit_Pattern_Any(loc~)
    Array(pats~, loc~) => env.0.visit_Pattern_Array(pats~, loc~)
    Constant(c~, loc~) => env.0.visit_Pattern_Constant(c~, loc~)
    Constraint(pat~, ty~, loc~) =>
      env.0.visit_Pattern_Constraint(pat~, ty~, loc~)
    Constr(constr~, args~, is_open~, loc~) =>
      env.0.visit_Pattern_Constr(constr~, args~, is_open~, loc~)
    Or(pat1~, pat2~, loc~) => env.0.visit_Pattern_Or(pat1~, pat2~, loc~)
    Tuple(pats~, loc~) => env.0.visit_Pattern_Tuple(pats~, loc~)
    Var(binder) => env.0.visit_Pattern_Var(binder)
    Record(fields~, is_closed~, loc~) =>
      env.0.visit_Pattern_Record(fields~, is_closed~, loc~)
    Map(elems~, is_closed~, loc~) =>
      env.0.visit_Pattern_Map(elems~, is_closed~, loc~)
    Range(lhs~, rhs~, kind~, loc~) =>
      env.0.visit_Pattern_Range(lhs~, rhs~, kind~, loc~)
    SpecialConstr(binder~, args~, loc~) =>
      env.0.visit_Pattern_SpecialConstr(binder~, args~, loc~)
  }
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Pattern_Alias(
  env,
  pat~,
  alias_~,
  loc~,
) {
  ignore(loc)
  env.0.visit_Pattern(pat)
  env.0.visit_Binder(alias_)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Pattern_Any(
  _,
  loc~,
) {
  ignore(loc)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Pattern_Array(
  env,
  pats~,
  loc~,
) {
  ignore(loc)
  env.0.visit_ArrayPatterns(pats)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Pattern_Constant(
  env,
  c~,
  loc~,
) {
  ignore(loc)
  env.0.visit_Constant(c)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Pattern_Constraint(
  env,
  pat~,
  ty~,
  loc~,
) {
  ignore(loc)
  env.0.visit_Pattern(pat)
  env.0.visit_Type(ty)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Pattern_Constr(
  env,
  constr~,
  args~,
  is_open~,
  loc~,
) {
  ignore(is_open)
  ignore(loc)
  env.0.visit_Constructor(constr)
  args.each(args => args.each(x => env.0.visit_ConstrPatArg(x)))
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Pattern_Or(
  env,
  pat1~,
  pat2~,
  loc~,
) {
  ignore(loc)
  env.0.visit_Pattern(pat1)
  env.0.visit_Pattern(pat2)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Pattern_Tuple(
  env,
  pats~,
  loc~,
) {
  ignore(loc)
  pats.each(x => env.0.visit_Pattern(x))
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Pattern_Var(
  env,
  binder,
) {
  env.0.visit_Binder(binder)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Pattern_Record(
  env,
  fields~,
  is_closed~,
  loc~,
) {
  ignore(is_closed)
  ignore(loc)
  fields.each(x => env.0.visit_FieldPat(x))
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Pattern_Map(
  env,
  elems~,
  is_closed~,
  loc~,
) {
  ignore(is_closed)
  ignore(loc)
  elems.each(x => env.0.visit_MapPatElem(x))
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Pattern_Range(
  env,
  lhs~,
  rhs~,
  kind~,
  loc~,
) {
  ignore(loc)
  env.0.visit_Pattern(lhs)
  env.0.visit_Pattern(rhs)
  env.0.visit_RangeKind(kind)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Pattern_SpecialConstr(
  env,
  binder~,
  args~,
  loc~,
) {
  ignore(loc)
  env.0.visit_Binder(binder)
  args.each(x => env.0.visit_ConstrPatArg(x))
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_LocalTypeDecl(
  env,
  localtype_decl,
) {
  env.0.visit_TypeDesc(localtype_decl.components)
  localtype_decl.deriving.each(x => env.0.visit_DerivingDirective(x))
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_DerivingDirective(
  env,
  deriving_directive,
) {
  env.0.visit_TypeName(deriving_directive.type_name)
  deriving_directive.args.each(x => env.0.visit_Argument(x))
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_TypeDecl(
  env,
  type_decl,
) {
  type_decl.params.each(x => env.0.visit_TypeDeclBinder(x))
  env.0.visit_TypeDesc(type_decl.components)
  type_decl.deriving.each(x => env.0.visit_DerivingDirective(x))
  env.0.visit_Visibility(type_decl.type_vis)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_FuncStubs(
  env,
  func_stubs,
) {
  match func_stubs {
    Import(module_name~, func_name~, language~) =>
      env.0.visit_FuncStubs_Import(module_name~, func_name~, language~)
    Embedded(language~, code~) =>
      env.0.visit_FuncStubs_Embedded(language~, code~)
  }
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_FuncStubs_Import(
  _,
  module_name~,
  func_name~,
  language~,
) {
  ignore(module_name)
  ignore(func_name)
  ignore(language)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_FuncStubs_Embedded(
  env,
  language~,
  code~,
) {
  ignore(language)
  env.0.visit_EmbeddedCode(code)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_EmbeddedCode(
  env,
  embedded_code,
) {
  match embedded_code {
    CodeString(string_literal) =>
      env.0.visit_EmbeddedCode_CodeString(string_literal)
    CodeMultilineString(strings) =>
      env.0.visit_EmbeddedCode_CodeMultilineString(strings)
  }
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_EmbeddedCode_CodeString(
  _,
  _,
) {
  ()
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_EmbeddedCode_CodeMultilineString(
  _,
  _,
) {
  ()
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_DeclBody(
  env,
  decl_body,
) {
  match decl_body {
    DeclBody(local_types~, expr~) =>
      env.0.visit_DeclBody_DeclBody(local_types~, expr~)
    DeclStubs(func_stubs) => env.0.visit_DeclBody_DeclStubs(func_stubs)
    DeclNone => ()
  }
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_DeclBody_DeclBody(
  env,
  local_types~,
  expr~,
) {
  local_types.each(x => env.0.visit_LocalTypeDecl(x))
  env.0.visit_Expr(expr)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_DeclBody_DeclStubs(
  env,
  func_stubs,
) {
  env.0.visit_FuncStubs(func_stubs)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_FunDecl(
  env,
  fun_decl,
) {
  fun_decl.type_name.each(x => env.0.visit_TypeName(x))
  env.0.visit_Binder(fun_decl.name)
  fun_decl.decl_params.each(decl_params => {
    decl_params.each(x => env.0.visit_Parameter(x))
  })
  fun_decl.quantifiers.each(x => env.0.visit_TypeVarBinder(x))
  fun_decl.return_type.each(x => env.0.visit_Type(x))
  env.0.visit_ErrorType(fun_decl.error_type)
  env.0.visit_Visibility(fun_decl.vis)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_TraitMethodDecl(
  env,
  trait_method_decl,
) {
  env.0.visit_Binder(trait_method_decl.name)
  trait_method_decl.quantifiers.each(x => env.0.visit_TypeVarBinder(x))
  trait_method_decl.params.each(param => env.0.visit_Parameter(param))
  trait_method_decl.return_type.each(x => env.0.visit_Type(x))
  env.0.visit_ErrorType(trait_method_decl.error_type)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_TraitDecl(
  env,
  trait_decl,
) {
  env.0.visit_Binder(trait_decl.name)
  trait_decl.supers.each(x => env.0.visit_TypeVarConstraint(x))
  trait_decl.methods.each(x => env.0.visit_TraitMethodDecl(x))
  env.0.visit_Visibility(trait_decl.vis)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_UsingKind(
  env,
  using_kind,
) {
  ignore(env)
  ignore(using_kind)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Impl_TopExpr(
  env,
  expr~,
  is_main~,
  local_types~,
  is_async~,
  loc~,
) {
  ignore(is_main)
  ignore(loc)
  ignore(is_async)
  env.0.visit_Expr(expr)
  local_types.each(x => env.0.visit_LocalTypeDecl(x))
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Impl_TopTest(
  env,
  expr~,
  name~,
  params~,
  local_types~,
  is_async~,
  loc~,
  attrs~,
  doc~,
) {
  ignore(name)
  ignore(loc)
  ignore(attrs)
  ignore(doc)
  ignore(is_async)
  env.0.visit_Expr(expr)
  params.each(params => params.each(param => env.0.visit_Parameter(param)))
  local_types.each(x => env.0.visit_LocalTypeDecl(x))
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Impl_TopTypeDef(
  env,
  type_decl,
) {
  env.0.visit_TypeDecl(type_decl)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Impl_TopFuncDef(
  env,
  fun_decl~,
  decl_body~,
  loc~,
) {
  ignore(loc)
  env.0.visit_FunDecl(fun_decl)
  env.0.visit_DeclBody(decl_body)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Impl_TopFuncAlias(
  env,
  pkg~,
  type_name~,
  targets~,
  vis~,
  attrs~,
  is_list~,
  doc~,
  loc~,
) {
  ignore(attrs)
  ignore(is_list)
  ignore(doc)
  ignore(loc)
  pkg.each(x => env.0.visit_Label(x))
  type_name.each(x => env.0.visit_Label(x))
  targets.each(x => env.0.visit_AliasTarget(x))
  env.0.visit_Visibility(vis)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Impl_TopLetDef(
  env,
  binder~,
  ty~,
  expr~,
  vis~,
  is_constant~,
  loc~,
  attrs~,
  doc~,
  is_declare~,
) {
  ignore(is_constant)
  ignore(loc)
  ignore(attrs)
  ignore(doc)
  ignore(is_declare)
  env.0.visit_Binder(binder)
  ty.each(x => env.0.visit_Type(x))
  env.0.visit_Expr(expr)
  env.0.visit_Visibility(vis)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Impl_TopTrait(
  env,
  trait_decl,
) {
  env.0.visit_TraitDecl(trait_decl)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Impl_TopBatchTypeAlias(
  env,
  pkg~,
  targets~,
  vis~,
  loc~,
  attrs~,
  is_list~,
  doc~,
) {
  ignore(doc)
  ignore(is_list)
  ignore(attrs)
  ignore(loc)
  pkg.each(x => env.0.visit_Label(x))
  targets.each(x => env.0.visit_AliasTarget(x))
  env.0.visit_Visibility(vis)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Impl_TopBatchTraitAlias(
  env,
  pkg~,
  targets~,
  vis~,
  loc~,
  attrs~,
  is_list~,
  doc~,
) {
  ignore(loc)
  ignore(attrs)
  ignore(is_list)
  ignore(doc)
  pkg.each(x => env.0.visit_Label(x))
  targets.each(x => env.0.visit_AliasTarget(x))
  env.0.visit_Visibility(vis)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Impl_TopImpl(
  env,
  self_ty~,
  trait_~,
  method_name~,
  has_error~,
  quantifiers~,
  params~,
  ret_ty~,
  err_ty~,
  body~,
  vis~,
  loc~,
  attrs~,
  doc~,
) {
  ignore(loc)
  ignore(has_error)
  ignore(attrs)
  ignore(doc)
  self_ty.each(x => env.0.visit_Type(x))
  env.0.visit_TypeName(trait_)
  env.0.visit_Binder(method_name)
  quantifiers.each(x => env.0.visit_TypeVarBinder(x))
  params.each(param => env.0.visit_Parameter(param))
  ret_ty.each(x => env.0.visit_Type(x))
  env.0.visit_ErrorType(err_ty)
  env.0.visit_DeclBody(body)
  env.0.visit_Visibility(vis)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Impl_TopView(
  env,
  quantifiers~,
  source_ty~,
  view_type_name~,
  view_type_loc~,
  view_constrs~,
  view_func_name~,
  parameters~,
  params_loc~,
  body~,
  vis~,
  loc~,
  attrs~,
  doc~,
) {
  ignore(view_type_name)
  ignore(view_type_loc)
  ignore(params_loc)
  ignore(loc)
  ignore(attrs)
  ignore(doc)
  quantifiers.each(x => env.0.visit_TypeVarBinder(x))
  env.0.visit_Type(source_ty)
  view_constrs.each(x => env.0.visit_ConstrDecl(x))
  env.0.visit_Binder(view_func_name)
  parameters.each(param => env.0.visit_Parameter(param))
  env.0.visit_Expr(body)
  env.0.visit_Visibility(vis)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Impl_TopImplRelation(
  env,
  self_ty~,
  trait_~,
  quantifiers~,
  vis~,
  attrs~,
  loc~,
  doc~,
  is_declare~,
) {
  ignore(is_declare)
  ignore(attrs)
  ignore(loc)
  ignore(doc)
  env.0.visit_Type(self_ty)
  env.0.visit_TypeName(trait_)
  quantifiers.each(x => env.0.visit_TypeVarBinder(x))
  env.0.visit_Visibility(vis)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Impl_TopUsing(
  env,
  pkg~,
  names~,
  vis~,
  attrs~,
  loc~,
  doc~,
) {
  ignore(loc)
  ignore(attrs)
  ignore(doc)
  env.0.visit_Label(pkg)
  names.each(name => {
    env.0.visit_AliasTarget(name.0)
    env.0.visit_UsingKind(name.1)
  })
  env.0.visit_Visibility(vis)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Expr_Apply(
  env,
  func~,
  args~,
  attr~,
  loc~,
) {
  ignore(loc)
  ignore(attr)
  env.0.visit_Expr(func)
  args.each(x => env.0.visit_Argument(x))
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Expr_Infix(
  env,
  op~,
  lhs~,
  rhs~,
  loc~,
) {
  ignore(loc)
  env.0.visit_Var(op)
  env.0.visit_Expr(lhs)
  env.0.visit_Expr(rhs)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Expr_Unary(
  env,
  op~,
  expr~,
  loc~,
) {
  ignore(loc)
  env.0.visit_Var(op)
  env.0.visit_Expr(expr)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Expr_Array(
  env,
  exprs~,
  loc~,
) {
  ignore(loc)
  exprs.each(expr => env.0.visit_Expr(expr))
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Expr_ArraySpread(
  env,
  elems~,
  loc~,
) {
  ignore(loc)
  elems.each(fn(elem) {
    match elem {
      Regular(expr) => env.0.visit_Expr(expr)
      Spread(expr~, loc~) => {
        ignore(loc)
        env.0.visit_Expr(expr)
      }
    }
  })
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Expr_ArrayGet(
  env,
  array~,
  index~,
  loc~,
) {
  ignore(loc)
  env.0.visit_Expr(array)
  env.0.visit_Expr(index)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Expr_ArrayGetSlice(
  env,
  array~,
  start_index~,
  end_index~,
  index_loc~,
  loc~,
) {
  ignore(loc)
  ignore(index_loc)
  env.0.visit_Expr(array)
  start_index.each(e => env.0.visit_Expr(e))
  end_index.each(e => env.0.visit_Expr(e))
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Expr_ArraySet(
  env,
  array~,
  index~,
  value~,
  loc~,
) {
  ignore(loc)
  env.0.visit_Expr(index)
  env.0.visit_Expr(value)
  env.0.visit_Expr(array)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Expr_ArrayAugmentedSet(
  env,
  op~,
  array~,
  index~,
  value~,
  loc~,
) {
  ignore(loc)
  env.0.visit_Var(op)
  env.0.visit_Expr(array)
  env.0.visit_Expr(index)
  env.0.visit_Expr(value)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Expr_Constant(
  env,
  c~,
  loc~,
) {
  ignore(loc)
  env.0.visit_Constant(c)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Expr_MultilineString(
  env,
  elems~,
  loc~,
) {
  ignore(loc)
  elems.each(elem => env.0.visit_MultilineStringElem(elem))
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Expr_Interp(
  env,
  elems~,
  loc~,
) {
  ignore(loc)
  elems.each(elem => env.0.visit_InterpElem(elem))
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Expr_Constraint(
  env,
  expr~,
  ty~,
  loc~,
) {
  ignore(loc)
  env.0.visit_Type(ty)
  env.0.visit_Expr(expr)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Expr_Constr(
  env,
  constr~,
  loc~,
) {
  ignore(loc)
  env.0.visit_Constructor(constr)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Expr_While(
  env,
  loop_cond~,
  loop_body~,
  while_else~,
  label~,
  loc~,
) {
  ignore(loc)
  env.0.visit_Expr(loop_cond)
  env.0.visit_Expr(loop_body)
  while_else.each(else_block => env.0.visit_Expr(else_block))
  label.each(x => env.0.visit_Label(x))
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Expr_Function(
  env,
  func~,
  loc~,
) {
  ignore(loc)
  env.0.visit_Func(func)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Expr_Ident(
  env,
  id~,
  loc~,
) {
  ignore(loc)
  env.0.visit_Var(id)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Expr_If(
  env,
  cond~,
  ifso~,
  ifnot~,
  loc~,
) {
  ignore(loc)
  env.0.visit_Expr(cond)
  env.0.visit_Expr(ifso)
  ifnot.each(expr => env.0.visit_Expr(expr))
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Expr_Guard(
  env,
  cond~,
  otherwise~,
  body~,
  loc~,
) {
  ignore(loc)
  env.0.visit_Expr(cond)
  env.0.visit_Expr(body)
  otherwise.each(expr => env.0.visit_Expr(expr))
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Expr_Is(
  env,
  expr~,
  pat~,
  loc~,
) {
  ignore(loc)
  env.0.visit_Expr(expr)
  env.0.visit_Pattern(pat)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Expr_Defer(
  env,
  expr~,
  body~,
  loc~,
) {
  ignore(loc)
  env.0.visit_Expr(expr)
  env.0.visit_Expr(body)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Expr_LetFn(
  env,
  name~,
  func~,
  body~,
  loc~,
) {
  ignore(loc)
  env.0.visit_Binder(name)
  env.0.visit_Func(func)
  env.0.visit_Expr(body)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Expr_LetRec(
  env,
  bindings~,
  body~,
  loc~,
) {
  ignore(loc)
  bindings.each(binding => {
    match binding {
      (name, func) => {
        env.0.visit_Binder(name)
        env.0.visit_Func(func)
      }
    }
  })
  env.0.visit_Expr(body)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Expr_LetAnd(
  env,
  bindings~,
  body~,
  loc~,
) {
  ignore(loc)
  bindings.each(binding => {
    match binding {
      (binder, ty, func) => {
        env.0.visit_Binder(binder)
        ty.each(x => env.0.visit_Type(x))
        env.0.visit_Func(func)
      }
    }
  })
  env.0.visit_Expr(body)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Expr_Let(
  env,
  pattern~,
  expr~,
  body~,
  loc~,
) {
  ignore(loc)
  env.0.visit_Pattern(pattern)
  env.0.visit_Expr(expr)
  env.0.visit_Expr(body)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Expr_Sequence(
  env,
  exprs~,
  last_expr~,
  loc~,
) {
  ignore(loc)
  exprs.each(expr => env.0.visit_Expr(expr))
  env.0.visit_Expr(last_expr)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Expr_Tuple(
  env,
  exprs~,
  loc~,
) {
  ignore(loc)
  exprs.each(expr => env.0.visit_Expr(expr))
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Expr_Record(
  env,
  type_name~,
  fields~,
  trailing~,
  loc~,
) {
  ignore(loc)
  type_name.each(x => env.0.visit_TypeName(x))
  fields.each(x => env.0.visit_FieldDef(x))
  env.0.visit_TrailingMark(trailing)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Expr_RecordUpdate(
  env,
  type_name~,
  record~,
  fields~,
  loc~,
) {
  ignore(loc)
  type_name.each(x => env.0.visit_TypeName(x))
  fields.each(x => env.0.visit_FieldDef(x))
  env.0.visit_Expr(record)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Expr_Field(
  env,
  record~,
  accessor~,
  loc~,
) {
  ignore(loc)
  env.0.visit_Expr(record)
  env.0.visit_Accessor(accessor)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Expr_Method(
  env,
  type_name~,
  method_name~,
  loc~,
) {
  ignore(loc)
  env.0.visit_TypeName(type_name)
  env.0.visit_Label(method_name)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Expr_DotApply(
  env,
  self~,
  method_name~,
  args~,
  return_self~,
  attr~,
  loc~,
) {
  ignore(return_self)
  ignore(attr)
  ignore(loc)
  env.0.visit_Expr(self)
  env.0.visit_Label(method_name)
  args.each(x => env.0.visit_Argument(x))
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Expr_As(
  env,
  expr~,
  trait_~,
  loc~,
) {
  ignore(loc)
  env.0.visit_TypeName(trait_)
  env.0.visit_Expr(expr)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Expr_Mutate(
  env,
  record~,
  accessor~,
  field~,
  augmented_by~,
  loc~,
) {
  ignore(loc)
  env.0.visit_Expr(record)
  env.0.visit_Accessor(accessor)
  env.0.visit_Expr(field)
  augmented_by.each(x => env.0.visit_Var(x))
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Expr_Match(
  env,
  expr~,
  cases~,
  match_loc~,
  loc~,
) {
  ignore(match_loc)
  ignore(loc)
  env.0.visit_Expr(expr)
  cases.each(x => env.0.visit_Case(x))
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Expr_LetMut(
  env,
  binder~,
  ty~,
  expr~,
  body~,
  loc~,
) {
  ignore(loc)
  env.0.visit_Binder(binder)
  ty.each(x => env.0.visit_Type(x))
  env.0.visit_Expr(expr)
  env.0.visit_Expr(body)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Expr_Pipe(
  env,
  lhs~,
  rhs~,
  loc~,
) {
  ignore(loc)
  env.0.visit_Expr(lhs)
  env.0.visit_Expr(rhs)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Expr_Assign(
  env,
  var_~,
  expr~,
  augmented_by~,
  loc~,
) {
  ignore(loc)
  env.0.visit_Var(var_)
  augmented_by.each(x => env.0.visit_Var(x))
  env.0.visit_Expr(expr)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Expr_Hole(
  env,
  loc~,
  kind~,
) {
  ignore(loc)
  env.0.visit_Hole(kind)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Expr_Return(
  env,
  return_value~,
  loc~,
) {
  ignore(loc)
  return_value.each(value => env.0.visit_Expr(value))
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Expr_Raise(
  env,
  err_value~,
  loc~,
) {
  ignore(loc)
  env.0.visit_Expr(err_value)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Expr_Unit(
  _,
  loc~,
  faked~,
) {
  ignore(loc)
  ignore(faked)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Expr_Break(
  env,
  arg~,
  label~,
  loc~,
) {
  ignore(loc)
  label.each(x => env.0.visit_Label(x))
  arg.each(x => env.0.visit_Expr(x))
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Expr_Continue(
  env,
  args~,
  label~,
  loc~,
) {
  ignore(loc)
  args.each(arg => env.0.visit_Expr(arg))
  label.each(x => env.0.visit_Label(x))
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Expr_Loop(
  env,
  arg~,
  body~,
  label~,
  loop_loc~,
  loc~,
) {
  ignore(loop_loc)
  ignore(loc)
  env.0.visit_Expr(arg)
  body.each(x => env.0.visit_Case(x))
  label.each(x => env.0.visit_Label(x))
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Expr_For(
  env,
  binders~,
  condition~,
  continue_block~,
  body~,
  for_else~,
  label~,
  loc~,
  where_clause~,
) {
  ignore(loc)
  binders.each(p => {
    env.0.visit_Binder(p.0)
    env.0.visit_Expr(p.1)
  })
  condition.each(cond => env.0.visit_Expr(cond))
  continue_block.each(p => {
    env.0.visit_Binder(p.0)
    env.0.visit_Expr(p.1)
  })
  env.0.visit_Expr(body)
  for_else.each(else_block => env.0.visit_Expr(else_block))
  label.each(x => env.0.visit_Label(x))
  where_clause.each(x => env.0.visit_WhereClause(x))
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Expr_ForEach(
  env,
  binders~,
  expr~,
  body~,
  else_block~,
  label~,
  loc~,
) {
  ignore(loc)
  binders.each(binder => binder.each(x => env.0.visit_Binder(x)))
  env.0.visit_Expr(expr)
  env.0.visit_Expr(body)
  else_block.each(else_block => env.0.visit_Expr(else_block))
  label.each(x => env.0.visit_Label(x))
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Expr_Try(
  env,
  body~,
  catch_~,
  catch_all~,
  try_else~,
  has_try~,
  try_loc~,
  catch_loc~,
  else_loc~,
  loc~,
) {
  ignore(loc)
  ignore(try_loc)
  ignore(catch_loc)
  ignore(else_loc)
  ignore(has_try)
  ignore(catch_all)
  env.0.visit_Expr(body)
  catch_.each(x => env.0.visit_Case(x))
  try_else.each(try_else => try_else.each(x => env.0.visit_Case(x)))
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Expr_TryOperator(
  env,
  body~,
  kind~,
  try_loc~,
  loc~,
) {
  ignore(loc)
  ignore(try_loc)
  env.0.visit_Expr(body)
  env.0.visit_TryOperatorKind(kind)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Expr_Map(
  env,
  elems~,
  loc~,
) {
  ignore(loc)
  elems.each(x => env.0.visit_MapExprElem(x))
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Expr_Group(
  env,
  expr~,
  group~,
  loc~,
) {
  ignore(loc)
  env.0.visit_Expr(expr)
  env.0.visit_Group(group)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Expr_StaticAssert(
  env,
  asserts~,
  body~,
) {
  asserts.each(x => env.0.visit_StaticAssertion(x))
  env.0.visit_Expr(body)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Expr(
  env,
  expr,
) {
  match expr {
    Apply(func~, args~, attr~, loc~) =>
      env.0.visit_Expr_Apply(func~, args~, attr~, loc~)
    Infix(op~, lhs~, rhs~, loc~) =>
      env.0.visit_Expr_Infix(op~, lhs~, rhs~, loc~)
    Unary(op~, expr~, loc~) => env.0.visit_Expr_Unary(op~, expr~, loc~)
    Array(exprs~, loc~) => env.0.visit_Expr_Array(exprs~, loc~)
    ArraySpread(elems~, loc~) => env.0.visit_Expr_ArraySpread(elems~, loc~)
    ArrayGet(array~, index~, loc~) =>
      env.0.visit_Expr_ArrayGet(array~, index~, loc~)
    ArrayGetSlice(array~, start_index~, end_index~, index_loc~, loc~) =>
      env.0.visit_Expr_ArrayGetSlice(
        array~,
        start_index~,
        end_index~,
        index_loc~,
        loc~,
      )
    ArraySet(array~, index~, value~, loc~) =>
      env.0.visit_Expr_ArraySet(array~, index~, value~, loc~)
    ArrayAugmentedSet(op~, array~, index~, value~, loc~) =>
      env.0.visit_Expr_ArrayAugmentedSet(op~, array~, index~, value~, loc~)
    Constant(c~, loc~) => env.0.visit_Expr_Constant(c~, loc~)
    MultilineString(elems~, loc~) =>
      env.0.visit_Expr_MultilineString(elems~, loc~)
    Interp(elems~, loc~) => env.0.visit_Expr_Interp(elems~, loc~)
    Constraint(expr~, ty~, loc~) =>
      env.0.visit_Expr_Constraint(expr~, ty~, loc~)
    Constr(constr~, loc~) => env.0.visit_Expr_Constr(constr~, loc~)
    While(loop_cond~, loop_body~, while_else~, label~, loc~) =>
      env.0.visit_Expr_While(loop_cond~, loop_body~, while_else~, label~, loc~)
    Function(func~, loc~) => env.0.visit_Expr_Function(func~, loc~)
    Ident(id~, loc~) => env.0.visit_Expr_Ident(id~, loc~)
    If(cond~, ifso~, ifnot~, loc~) =>
      env.0.visit_Expr_If(cond~, ifso~, ifnot~, loc~)
    Guard(cond~, otherwise~, body~, loc~) =>
      env.0.visit_Expr_Guard(cond~, otherwise~, body~, loc~)
    Is(expr~, pat~, loc~) => env.0.visit_Expr_Is(expr~, pat~, loc~)
    IsLexMatch(expr~, strategy~, pat~, pat_loc~, loc~) =>
      env.0.visit_Expr_IsLexMatch(expr~, strategy~, pat~, pat_loc~, loc~)
    Defer(expr~, body~, loc~) => env.0.visit_Expr_Defer(expr~, body~, loc~)
    LetFn(name~, func~, body~, loc~) =>
      env.0.visit_Expr_LetFn(name~, func~, body~, loc~)
    LetRec(bindings~, body~, loc~) =>
      env.0.visit_Expr_LetRec(bindings~, body~, loc~)
    LetAnd(bindings~, body~, loc~) =>
      env.0.visit_Expr_LetAnd(bindings~, body~, loc~)
    Let(pattern~, expr~, body~, loc~) =>
      env.0.visit_Expr_Let(pattern~, expr~, body~, loc~)
    Sequence(exprs~, last_expr~, loc~) =>
      env.0.visit_Expr_Sequence(exprs~, last_expr~, loc~)
    Tuple(exprs~, loc~) => env.0.visit_Expr_Tuple(exprs~, loc~)
    Record(type_name~, fields~, trailing~, loc~) =>
      env.0.visit_Expr_Record(type_name~, fields~, trailing~, loc~)
    RecordUpdate(type_name~, record~, fields~, loc~) =>
      env.0.visit_Expr_RecordUpdate(type_name~, record~, fields~, loc~)
    Field(record~, accessor~, loc~) =>
      env.0.visit_Expr_Field(record~, accessor~, loc~)
    Method(type_name~, method_name~, loc~) =>
      env.0.visit_Expr_Method(type_name~, method_name~, loc~)
    DotApply(self~, method_name~, args~, return_self~, attr~, loc~) =>
      env.0.visit_Expr_DotApply(
        self~,
        method_name~,
        args~,
        return_self~,
        attr~,
        loc~,
      )
    As(expr~, trait_~, loc~) => env.0.visit_Expr_As(expr~, trait_~, loc~)
    Mutate(record~, accessor~, field~, augmented_by~, loc~) =>
      env.0.visit_Expr_Mutate(record~, accessor~, field~, augmented_by~, loc~)
    Match(expr~, cases~, match_loc~, loc~) =>
      env.0.visit_Expr_Match(expr~, cases~, match_loc~, loc~)
    LexMatch(strategy~, expr~, match_loc~, cases~, loc~) =>
      env.0.visit_Expr_LexMatch(strategy~, expr~, match_loc~, cases~, loc~)
    LetMut(binder~, ty~, expr~, body~, loc~) =>
      env.0.visit_Expr_LetMut(binder~, ty~, expr~, body~, loc~)
    Pipe(lhs~, rhs~, loc~) => env.0.visit_Expr_Pipe(lhs~, rhs~, loc~)
    Assign(var_~, expr~, augmented_by~, loc~) =>
      env.0.visit_Expr_Assign(var_~, expr~, augmented_by~, loc~)
    Hole(loc~, kind~) => env.0.visit_Expr_Hole(loc~, kind~)
    Return(return_value~, loc~) => env.0.visit_Expr_Return(return_value~, loc~)
    Raise(err_value~, loc~) => env.0.visit_Expr_Raise(err_value~, loc~)
    Unit(loc~, faked~) => env.0.visit_Expr_Unit(loc~, faked~)
    Break(arg~, label~, loc~) => env.0.visit_Expr_Break(arg~, label~, loc~)
    Continue(args~, label~, loc~) =>
      env.0.visit_Expr_Continue(args~, label~, loc~)
    Loop(arg~, body~, label~, loop_loc~, loc~) =>
      env.0.visit_Expr_Loop(arg~, body~, label~, loop_loc~, loc~)
    For(
      binders~,
      condition~,
      continue_block~,
      body~,
      for_else~,
      label~,
      loc~,
      where_clause~
    ) =>
      env.0.visit_Expr_For(
        binders~,
        condition~,
        continue_block~,
        body~,
        for_else~,
        label~,
        loc~,
        where_clause~,
      )
    ForEach(binders~, expr~, body~, else_block~, label~, loc~) =>
      env.0.visit_Expr_ForEach(
        binders~,
        expr~,
        body~,
        else_block~,
        label~,
        loc~,
      )
    Try(
      body~,
      catch_~,
      catch_all~,
      try_else~,
      has_try~,
      try_loc~,
      catch_loc~,
      else_loc~,
      loc~
    ) =>
      env.0.visit_Expr_Try(
        body~,
        catch_~,
        catch_all~,
        try_else~,
        has_try~,
        try_loc~,
        catch_loc~,
        else_loc~,
        loc~,
      )
    TryOperator(body~, kind~, try_loc~, loc~) =>
      env.0.visit_Expr_TryOperator(body~, kind~, try_loc~, loc~)
    Map(elems~, loc~) => env.0.visit_Expr_Map(elems~, loc~)
    Group(expr~, group~, loc~) => env.0.visit_Expr_Group(expr~, group~, loc~)
    StaticAssert(asserts~, body~) =>
      env.0.visit_Expr_StaticAssert(asserts~, body~)
  }
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Expr_IsLexMatch(
  env,
  expr~,
  strategy~,
  pat~,
  pat_loc~,
  loc~,
) {
  ignore(pat_loc)
  ignore(loc)
  strategy.each(strategy => env.0.visit_Label(strategy))
  pat.each(pat => env.0.visit_LexTopPattern(pat))
  env.0.visit_Expr(expr)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_Expr_LexMatch(
  env,
  strategy~,
  expr~,
  match_loc~,
  cases~,
  loc~,
) {
  ignore(match_loc)
  ignore(loc)
  strategy.each(strategy => env.0.visit_Label(strategy))
  env.0.visit_Expr(expr)
  cases.each(case => env.0.visit_LexCase(case))
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_LexCase(
  env,
  lex_case,
) {
  lex_case.pat.each(pat => env.0.visit_LexTopPattern(pat))
  env.0.visit_Expr(lex_case.body)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_LexTopPattern(
  env,
  lex_top_pattern,
) {
  match lex_top_pattern {
    Pattern(lex_pattern) => env.0.visit_LexTopPattern_Pattern(lex_pattern)
    Binder(binder) => env.0.visit_LexTopPattern_Binder(binder)
    Wildcard(loc~) => env.0.visit_LexTopPattern_Wildcard(loc~)
  }
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_LexTopPattern_Pattern(
  env,
  lex_pattern,
) {
  env.0.visit_LexPattern(lex_pattern)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_LexTopPattern_Binder(
  env,
  binder,
) {
  env.0.visit_Binder(binder)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_LexTopPattern_Wildcard(
  env,
  loc~,
) {
  ignore(env)
  ignore(loc)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_LexPattern(
  env,
  lex_pattern,
) {
  match lex_pattern {
    Alias(pat~, binder~, loc~) =>
      env.0.visit_LexPattern_Alias(pat~, binder~, loc~)
    RegexInterp(elems~, loc~) =>
      env.0.visit_LexPattern_RegexInterp(elems~, loc~)
    Regex(lit~, loc~) => env.0.visit_LexPattern_Regex(lit~, loc~)
    Sequence(pats~, loc~) => env.0.visit_LexPattern_Sequence(pats~, loc~)
  }
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_LexPattern_Regex(
  env,
  lit~,
  loc~,
) {
  ignore(env)
  ignore(lit)
  ignore(loc)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_LexPattern_RegexInterp(
  env,
  elems~,
  loc~,
) {
  ignore(loc)
  elems.each(elem => env.0.visit_InterpElem(elem))
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_LexPattern_Alias(
  env,
  pat~,
  binder~,
  loc~,
) {
  ignore(loc)
  env.0.visit_LexPattern(pat)
  env.0.visit_Binder(binder)
}

///|
pub impl[T : IterVisitor] IterVisitor for IterVisitorBase[T] with visit_LexPattern_Sequence(
  env,
  pats~,
  loc~,
) {
  ignore(loc)
  pats.each(pat => env.0.visit_LexPattern(pat))
}

///|
fn[T] Option::each(self : T?, f : (T) -> Unit raise?) -> Unit raise? {
  match self {
    Some(t) => f(t)
    None => ()
  }
}
