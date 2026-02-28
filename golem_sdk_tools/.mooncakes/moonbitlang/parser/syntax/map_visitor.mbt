///|
pub(open) trait MapVisitor {
  base(Self) -> MapVisitorBase[Self] = _

  // Visibility
  visit_Visibility(Self, Visibility) -> Visibility = _
  visit_Visibility_Pub(Self, attr~ : String?, loc~ : Location) -> Visibility = _
  visit_Visibility_Priv(Self, loc~ : Location) -> Visibility = _

  // Constant
  visit_Constant(Self, Constant) -> Constant = _

  // Label
  visit_Label(Self, Label) -> Label = _

  // ConstrName
  visit_ConstrName(Self, ConstrName) -> ConstrName = _

  // LongIdent
  visit_LongIdent(Self, LongIdent) -> LongIdent = _
  visit_LongIdent_Ident(Self, name~ : String) -> LongIdent = _
  visit_LongIdent_Dot(Self, pkg~ : String, id~ : String) -> LongIdent = _

  // TypeName
  visit_TypeName(Self, TypeName) -> TypeName = _

  // ConstrId
  visit_ConstrId(Self, ConstrId) -> ConstrId = _

  // Type
  visit_Type(Self, Type) -> Type = _
  visit_Type_Any(Self, loc~ : Location) -> Type = _
  visit_Type_Arrow(
    Self,
    args~ : @list.List[Type],
    res~ : Type,
    err~ : ErrorType,
    is_async~ : Location?,
    loc~ : Location,
  ) -> Type = _
  visit_Type_Tuple(Self, tys~ : @list.List[Type], loc~ : Location) -> Type = _
  visit_Type_Name(
    Self,
    constr_id~ : ConstrId,
    tys~ : @list.List[Type],
    loc~ : Location,
  ) -> Type = _
  visit_Type_Option(Self, ty~ : Type, loc~ : Location, question_loc~ : Location) -> Type = _
  visit_Type_Object(Self, ConstrId) -> Type = _

  // ErrorType
  visit_ErrorType(Self, ErrorType) -> ErrorType = _
  visit_ErrorType_ErrorType(Self, ty~ : Type) -> ErrorType = _
  visit_ErrorType_DefaultErrorType(Self, loc~ : Location) -> ErrorType = _
  visit_ErrorType_Noraise(Self, loc~ : Location) -> ErrorType = _
  visit_ErrorType_MaybeError(Self, ty~ : Type) -> ErrorType = _

  // ConstrParam
  visit_ConstrParam(Self, ConstrParam) -> ConstrParam = _

  // ConstrDecl
  visit_ConstrDecl(Self, ConstrDecl) -> ConstrDecl = _

  // ExceptionDecl
  visit_ExceptionDecl(Self, ExceptionDecl) -> ExceptionDecl = _
  visit_ExceptionDecl_SinglePayload(Self, Type) -> ExceptionDecl = _
  visit_ExceptionDecl_EnumPayload(Self, @list.List[ConstrDecl]) -> ExceptionDecl = _

  // FieldName
  visit_FieldName(Self, FieldName) -> FieldName = _

  // FieldDecl
  visit_FieldDecl(Self, FieldDecl) -> FieldDecl = _

  // TypeDesc
  visit_TypeDesc(Self, TypeDesc) -> TypeDesc = _
  visit_TypeDesc_Newtype(Self, Type) -> TypeDesc = _
  visit_TypeDesc_Error(Self, ExceptionDecl) -> TypeDesc = _
  visit_TypeDesc_Variant(Self, @list.List[ConstrDecl]) -> TypeDesc = _
  visit_TypeDesc_Record(Self, @list.List[FieldDecl]) -> TypeDesc = _
  visit_TypeDesc_TupleStruct(Self, @list.List[Type]) -> TypeDesc = _
  visit_TypeDesc_Alias(Self, Type) -> TypeDesc = _

  // Hole
  visit_Hole(Self, Hole) -> Hole = _

  // ArgumentKind
  visit_ArgumentKind(Self, ArgumentKind) -> ArgumentKind = _
  visit_ArgumentKind_Labelled(Self, Label) -> ArgumentKind = _
  visit_ArgumentKind_LabelledPun(Self, Label) -> ArgumentKind = _
  visit_ArgumentKind_LabelledOption(
    Self,
    label~ : Label,
    question_loc~ : Location,
  ) -> ArgumentKind = _
  visit_ArgumentKind_LabelledOptionPun(
    Self,
    label~ : Label,
    question_loc~ : Location,
  ) -> ArgumentKind = _

  // FnKind
  visit_FnKind(Self, FnKind) -> FnKind = _

  // Group
  visit_Group(Self, Group) -> Group = _

  // TrailingMark
  visit_TrailingMark(Self, TrailingMark) -> TrailingMark = _

  // ApplyAttr
  visit_ApplyAttr(Self, ApplyAttr) -> ApplyAttr = _

  // TypeVarConstraint
  visit_TypeVarConstraint(Self, TypeVarConstraint) -> TypeVarConstraint = _

  // TypeVarBinder
  visit_TypeVarBinder(Self, TypeVarBinder) -> TypeVarBinder = _

  // TypeDeclBinder
  visit_TypeDeclBinder(Self, TypeDeclBinder) -> TypeDeclBinder = _

  // Binder
  visit_Binder(Self, Binder) -> Binder = _

  // Var
  visit_Var(Self, Var) -> Var = _

  // ConstructorExtraInfo
  visit_ConstructorExtraInfo(Self, ConstructorExtraInfo) -> ConstructorExtraInfo = _
  visit_ConstructorExtraInfo_TypeName(Self, TypeName) -> ConstructorExtraInfo = _
  visit_ConstructorExtraInfo_Package(Self, String) -> ConstructorExtraInfo = _

  // Constructor
  visit_Constructor(Self, Constructor) -> Constructor = _

  // Accessor
  visit_Accessor(Self, Accessor) -> Accessor = _
  visit_Accessor_Label(Self, Label) -> Accessor = _
  visit_Accessor_Index(Self, tuple_index~ : Int, loc~ : Location) -> Accessor = _
  visit_Accessor_Newtype(Self, loc~ : Location) -> Accessor = _

  // AliasTarget
  visit_AliasTarget(Self, AliasTarget) -> AliasTarget = _

  // Argument
  visit_Argument(Self, Argument) -> Argument = _

  // Parameter
  visit_Parameter(Self, Parameter) -> Parameter = _
  visit_Parameter_DiscardPositional(Self, ty~ : Type?, loc~ : Location) -> Parameter = _
  visit_Parameter_Positional(Self, binder~ : Binder, ty~ : Type?) -> Parameter = _
  visit_Parameter_Labelled(Self, binder~ : Binder, ty~ : Type?) -> Parameter = _
  visit_Parameter_Optional(Self, binder~ : Binder, default~ : Expr, ty~ : Type?) -> Parameter = _
  visit_Parameter_QuestionOptional(Self, binder~ : Binder, ty~ : Type?) -> Parameter = _

  // Case
  visit_Case(Self, Case) -> Case = _

  // LexCase
  visit_LexCase(Self, LexCase) -> LexCase = _

  // LexTopPattern
  visit_LexTopPattern(Self, LexTopPattern) -> LexTopPattern = _
  visit_LexTopPattern_Pattern(Self, LexPattern) -> LexTopPattern = _
  visit_LexTopPattern_Binder(Self, Binder) -> LexTopPattern = _
  visit_LexTopPattern_Wildcard(Self, loc~ : Location) -> LexTopPattern = _

  // LexPattern
  visit_LexPattern(Self, LexPattern) -> LexPattern = _
  visit_LexPattern_Regex(Self, lit~ : String, loc~ : Location) -> LexPattern = _
  visit_LexPattern_RegexInterp(
    Self,
    elems~ : @list.List[InterpElem],
    loc~ : Location,
  ) -> LexPattern = _
  visit_LexPattern_Alias(
    Self,
    pat~ : LexPattern,
    binder~ : Binder,
    loc~ : Location,
  ) -> LexPattern = _
  visit_LexPattern_Sequence(
    Self,
    pats~ : @list.List[LexPattern],
    loc~ : Location,
  ) -> LexPattern = _

  // SpreadableElem
  visit_SpreadableElem(Self, SpreadableElem) -> SpreadableElem = _
  visit_SpreadableElem_Regular(Self, Expr) -> SpreadableElem = _
  visit_SpreadableElem_Spread(Self, expr~ : Expr, loc~ : Location) -> SpreadableElem = _

  // MapExprElem
  visit_MapExprElem(Self, MapExprElem) -> MapExprElem = _

  // StaticAssertion
  visit_StaticAssertion(Self, StaticAssertion) -> StaticAssertion = _

  // Func
  visit_Func(Self, Func) -> Func = _

  // FieldDef
  visit_FieldDef(Self, FieldDef) -> FieldDef = _

  // InterpElem
  visit_InterpElem(Self, InterpElem) -> InterpElem = _
  visit_InterpElem_Literal(Self, repr~ : StringLiteral, loc~ : Location) -> InterpElem = _
  visit_InterpElem_Expr(Self, expr~ : Expr, loc~ : Location) -> InterpElem = _
  visit_InterpElem_Source(Self, @tokens.InterpSource) -> InterpElem = _

  // MultilineStringElem
  visit_MultilineStringElem(Self, MultilineStringElem) -> MultilineStringElem = _
  visit_MultilineStringElem_String(Self, String) -> MultilineStringElem = _
  visit_MultilineStringElem_Interp(Self, @list.List[InterpElem]) -> MultilineStringElem = _

  // TryOperatorKind
  visit_TryOperatorKind(Self, TryOperatorKind) -> TryOperatorKind = _

  // DotDotBinder
  visit_DotDotBinder(Self, DotDotBinder) -> DotDotBinder = _
  visit_DotDotBinder_BinderAs(Self, Binder) -> DotDotBinder = _
  visit_DotDotBinder_Binder(Self, Binder) -> DotDotBinder = _

  // ArrayPattern
  visit_ArrayPattern(Self, ArrayPattern) -> ArrayPattern = _
  visit_ArrayPattern_Pattern(Self, Pattern) -> ArrayPattern = _
  visit_ArrayPattern_StringSpread(Self, str~ : StringLiteral, loc~ : Location) -> ArrayPattern = _
  visit_ArrayPattern_BytesSpread(Self, bytes~ : BytesLiteral, loc~ : Location) -> ArrayPattern = _
  visit_ArrayPattern_ConstSpread(
    Self,
    binder~ : Binder,
    pkg~ : String?,
    loc~ : Location,
  ) -> ArrayPattern = _

  // ArrayPatterns
  visit_ArrayPatterns(Self, ArrayPatterns) -> ArrayPatterns = _
  visit_ArrayPatterns_Closed(Self, @list.List[ArrayPattern]) -> ArrayPatterns = _
  visit_ArrayPatterns_Open(
    Self,
    @list.List[ArrayPattern],
    @list.List[ArrayPattern],
    DotDotBinder,
  ) -> ArrayPatterns = _

  // WhereClause
  visit_WhereClause(Self, WhereClause) -> WhereClause = _

  // FieldPat
  visit_FieldPat(Self, FieldPat) -> FieldPat = _

  // ConstrPatArg
  visit_ConstrPatArg(Self, ConstrPatArg) -> ConstrPatArg = _

  // MapPatElem
  visit_MapPatElem(Self, MapPatElem) -> MapPatElem = _

  // RangeKind
  visit_RangeKind(Self, RangeKind) -> RangeKind = _

  // Pattern
  visit_Pattern(Self, Pattern) -> Pattern = _
  visit_Pattern_Alias(Self, pat~ : Pattern, alias_~ : Binder, loc~ : Location) -> Pattern = _
  visit_Pattern_Any(Self, loc~ : Location) -> Pattern = _
  visit_Pattern_Array(Self, pats~ : ArrayPatterns, loc~ : Location) -> Pattern = _
  visit_Pattern_Constant(Self, c~ : Constant, loc~ : Location) -> Pattern = _
  visit_Pattern_Constraint(Self, pat~ : Pattern, ty~ : Type, loc~ : Location) -> Pattern = _
  visit_Pattern_Constr(
    Self,
    constr~ : Constructor,
    args~ : @list.List[ConstrPatArg]?,
    is_open~ : Bool,
    loc~ : Location,
  ) -> Pattern = _
  visit_Pattern_Or(Self, pat1~ : Pattern, pat2~ : Pattern, loc~ : Location) -> Pattern = _
  visit_Pattern_Tuple(Self, pats~ : @list.List[Pattern], loc~ : Location) -> Pattern = _
  visit_Pattern_Var(Self, Binder) -> Pattern = _
  visit_Pattern_Record(
    Self,
    fields~ : @list.List[FieldPat],
    is_closed~ : Bool,
    loc~ : Location,
  ) -> Pattern = _
  visit_Pattern_Map(
    Self,
    elems~ : @list.List[MapPatElem],
    is_closed~ : Bool,
    loc~ : Location,
  ) -> Pattern = _
  visit_Pattern_Range(
    Self,
    lhs~ : Pattern,
    rhs~ : Pattern,
    kind~ : RangeKind,
    loc~ : Location,
  ) -> Pattern = _
  visit_Pattern_SpecialConstr(
    Self,
    binder~ : Binder,
    args~ : @list.List[ConstrPatArg],
    loc~ : Location,
  ) -> Pattern = _

  // LocalTypeDecl
  visit_LocalTypeDecl(Self, LocalTypeDecl) -> LocalTypeDecl = _

  // DerivingDirective
  visit_DerivingDirective(Self, DerivingDirective) -> DerivingDirective = _

  // TypeDecl
  visit_TypeDecl(Self, TypeDecl) -> TypeDecl = _

  // FuncStubs
  visit_FuncStubs(Self, FuncStubs) -> FuncStubs = _
  visit_FuncStubs_Import(
    Self,
    module_name~ : StringLiteral,
    func_name~ : StringLiteral,
    language~ : StringLiteral?,
  ) -> FuncStubs = _
  visit_FuncStubs_Embedded(
    Self,
    language~ : StringLiteral?,
    code~ : EmbeddedCode,
  ) -> FuncStubs = _

  // EmbeddedCode
  visit_EmbeddedCode(Self, EmbeddedCode) -> EmbeddedCode = _
  visit_EmbeddedCode_CodeString(Self, StringLiteral) -> EmbeddedCode = _
  visit_EmbeddedCode_CodeMultilineString(Self, @list.List[String]) -> EmbeddedCode = _

  // DeclBody
  visit_DeclBody(Self, DeclBody) -> DeclBody = _
  visit_DeclBody_DeclBody(
    Self,
    local_types~ : @list.List[LocalTypeDecl],
    expr~ : Expr,
  ) -> DeclBody = _
  visit_DeclBody_DeclStubs(Self, FuncStubs) -> DeclBody = _

  // FunDecl
  visit_FunDecl(Self, FunDecl) -> FunDecl = _

  // TraitMethodDecl
  visit_TraitMethodDecl(Self, TraitMethodDecl) -> TraitMethodDecl = _

  // TraitDecl
  visit_TraitDecl(Self, TraitDecl) -> TraitDecl = _

  // UsingKind
  visit_UsingKind(Self, UsingKind) -> UsingKind = _

  // Impl
  visit_Impl(Self, Impl) -> Impl = _
  visit_Impl_TopExpr(
    Self,
    expr~ : Expr,
    is_main~ : Bool,
    local_types~ : @list.List[LocalTypeDecl],
    is_async~ : Location?,
    loc~ : Location,
  ) -> Impl = _
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
  ) -> Impl = _
  visit_Impl_TopTypeDef(Self, TypeDecl) -> Impl = _
  visit_Impl_TopFuncDef(
    Self,
    fun_decl~ : FunDecl,
    decl_body~ : DeclBody,
    loc~ : Location,
  ) -> Impl = _
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
  ) -> Impl = _
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
  ) -> Impl = _
  visit_Impl_TopTrait(Self, TraitDecl) -> Impl = _
  visit_Impl_TopBatchTypeAlias(
    Self,
    pkg~ : Label?,
    targets~ : @list.List[AliasTarget],
    vis~ : Visibility,
    loc~ : Location,
    attrs~ : @list.List[Attribute],
    is_list~ : Bool,
    doc~ : DocString,
  ) -> Impl = _
  visit_Impl_TopBatchTraitAlias(
    Self,
    pkg~ : Label?,
    targets~ : @list.List[AliasTarget],
    vis~ : Visibility,
    loc~ : Location,
    attrs~ : @list.List[Attribute],
    is_list~ : Bool,
    doc~ : DocString,
  ) -> Impl = _
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
  ) -> Impl = _
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
  ) -> Impl = _
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
  ) -> Impl = _
  visit_Impl_TopUsing(
    Self,
    pkg~ : Label,
    names~ : @list.List[(AliasTarget, UsingKind)],
    vis~ : Visibility,
    attrs~ : @list.List[Attribute],
    loc~ : Location,
    doc~ : DocString,
  ) -> Impl = _

  // Expr
  visit_Expr(Self, Expr) -> Expr = _
  visit_Expr_Apply(
    Self,
    func~ : Expr,
    args~ : @list.List[Argument],
    attr~ : ApplyAttr,
    loc~ : Location,
  ) -> Expr = _
  visit_Expr_Infix(Self, op~ : Var, lhs~ : Expr, rhs~ : Expr, loc~ : Location) -> Expr = _
  visit_Expr_Unary(Self, op~ : Var, expr~ : Expr, loc~ : Location) -> Expr = _
  visit_Expr_Array(Self, exprs~ : @list.List[Expr], loc~ : Location) -> Expr = _
  visit_Expr_ArraySpread(
    Self,
    elems~ : @list.List[SpreadableElem],
    loc~ : Location,
  ) -> Expr = _
  visit_Expr_ArrayGet(Self, array~ : Expr, index~ : Expr, loc~ : Location) -> Expr = _
  visit_Expr_ArrayGetSlice(
    Self,
    array~ : Expr,
    start_index~ : Expr?,
    end_index~ : Expr?,
    index_loc~ : Location,
    loc~ : Location,
  ) -> Expr = _
  visit_Expr_ArraySet(
    Self,
    array~ : Expr,
    index~ : Expr,
    value~ : Expr,
    loc~ : Location,
  ) -> Expr = _
  visit_Expr_ArrayAugmentedSet(
    Self,
    op~ : Var,
    array~ : Expr,
    index~ : Expr,
    value~ : Expr,
    loc~ : Location,
  ) -> Expr = _
  visit_Expr_Constant(Self, c~ : Constant, loc~ : Location) -> Expr = _
  visit_Expr_MultilineString(
    Self,
    elems~ : @list.List[MultilineStringElem],
    loc~ : Location,
  ) -> Expr = _
  visit_Expr_Interp(Self, elems~ : @list.List[InterpElem], loc~ : Location) -> Expr = _
  visit_Expr_Constraint(Self, expr~ : Expr, ty~ : Type, loc~ : Location) -> Expr = _
  visit_Expr_Constr(Self, constr~ : Constructor, loc~ : Location) -> Expr = _
  visit_Expr_While(
    Self,
    loop_cond~ : Expr,
    loop_body~ : Expr,
    while_else~ : Expr?,
    label~ : Label?,
    loc~ : Location,
  ) -> Expr = _
  visit_Expr_Function(Self, func~ : Func, loc~ : Location) -> Expr = _
  visit_Expr_Ident(Self, id~ : Var, loc~ : Location) -> Expr = _
  visit_Expr_If(
    Self,
    cond~ : Expr,
    ifso~ : Expr,
    ifnot~ : Expr?,
    loc~ : Location,
  ) -> Expr = _
  visit_Expr_Guard(
    Self,
    cond~ : Expr,
    otherwise~ : Expr?,
    body~ : Expr,
    loc~ : Location,
  ) -> Expr = _
  visit_Expr_Is(Self, expr~ : Expr, pat~ : Pattern, loc~ : Location) -> Expr = _
  visit_Expr_IsLexMatch(
    Self,
    expr~ : Expr,
    strategy~ : Label?,
    pat~ : @list.List[LexTopPattern],
    pat_loc~ : Location,
    loc~ : Location,
  ) -> Expr = _
  visit_Expr_Defer(Self, expr~ : Expr, body~ : Expr, loc~ : Location) -> Expr = _
  visit_Expr_LetFn(
    Self,
    name~ : Binder,
    func~ : Func,
    body~ : Expr,
    loc~ : Location,
  ) -> Expr = _
  visit_Expr_LetRec(
    Self,
    bindings~ : @list.List[(Binder, Func)],
    body~ : Expr,
    loc~ : Location,
  ) -> Expr = _
  visit_Expr_LetAnd(
    Self,
    bindings~ : @list.List[(Binder, Type?, Func)],
    body~ : Expr,
    loc~ : Location,
  ) -> Expr = _
  visit_Expr_Let(
    Self,
    pattern~ : Pattern,
    expr~ : Expr,
    body~ : Expr,
    loc~ : Location,
  ) -> Expr = _
  visit_Expr_Sequence(
    Self,
    exprs~ : @list.List[Expr],
    last_expr~ : Expr,
    loc~ : Location,
  ) -> Expr = _
  visit_Expr_Tuple(Self, exprs~ : @list.List[Expr], loc~ : Location) -> Expr = _
  visit_Expr_Record(
    Self,
    type_name~ : TypeName?,
    fields~ : @list.List[FieldDef],
    trailing~ : TrailingMark,
    loc~ : Location,
  ) -> Expr = _
  visit_Expr_RecordUpdate(
    Self,
    type_name~ : TypeName?,
    record~ : Expr,
    fields~ : @list.List[FieldDef],
    loc~ : Location,
  ) -> Expr = _
  visit_Expr_Field(Self, record~ : Expr, accessor~ : Accessor, loc~ : Location) -> Expr = _
  visit_Expr_Method(
    Self,
    type_name~ : TypeName,
    method_name~ : Label,
    loc~ : Location,
  ) -> Expr = _
  visit_Expr_DotApply(
    Self,
    self~ : Expr,
    method_name~ : Label,
    args~ : @list.List[Argument],
    return_self~ : Bool,
    attr~ : ApplyAttr,
    loc~ : Location,
  ) -> Expr = _
  visit_Expr_As(Self, expr~ : Expr, trait_~ : TypeName, loc~ : Location) -> Expr = _
  visit_Expr_Mutate(
    Self,
    record~ : Expr,
    accessor~ : Accessor,
    field~ : Expr,
    augmented_by~ : Var?,
    loc~ : Location,
  ) -> Expr = _
  visit_Expr_Match(
    Self,
    expr~ : Expr,
    cases~ : @list.List[Case],
    match_loc~ : Location,
    loc~ : Location,
  ) -> Expr = _
  visit_Expr_LexMatch(
    Self,
    strategy~ : Label?,
    expr~ : Expr,
    match_loc~ : Location,
    cases~ : @list.List[LexCase],
    loc~ : Location,
  ) -> Expr = _
  visit_Expr_LetMut(
    Self,
    binder~ : Binder,
    ty~ : Type?,
    expr~ : Expr,
    body~ : Expr,
    loc~ : Location,
  ) -> Expr = _
  visit_Expr_Pipe(Self, lhs~ : Expr, rhs~ : Expr, loc~ : Location) -> Expr = _
  visit_Expr_Assign(
    Self,
    var_~ : Var,
    expr~ : Expr,
    augmented_by~ : Var?,
    loc~ : Location,
  ) -> Expr = _
  visit_Expr_Hole(Self, loc~ : Location, kind~ : Hole) -> Expr = _
  visit_Expr_Return(Self, return_value~ : Expr?, loc~ : Location) -> Expr = _
  visit_Expr_Raise(Self, err_value~ : Expr, loc~ : Location) -> Expr = _
  visit_Expr_Unit(Self, loc~ : Location, faked~ : Bool) -> Expr = _
  visit_Expr_Break(Self, arg~ : Expr?, label~ : Label?, loc~ : Location) -> Expr = _
  visit_Expr_Continue(
    Self,
    args~ : @list.List[Expr],
    label~ : Label?,
    loc~ : Location,
  ) -> Expr = _
  visit_Expr_Loop(
    Self,
    arg~ : Expr,
    body~ : @list.List[Case],
    label~ : Label?,
    loop_loc~ : Location,
    loc~ : Location,
  ) -> Expr = _
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
  ) -> Expr = _
  visit_Expr_ForEach(
    Self,
    binders~ : @list.List[Binder?],
    expr~ : Expr,
    body~ : Expr,
    else_block~ : Expr?,
    label~ : Label?,
    loc~ : Location,
  ) -> Expr = _
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
  ) -> Expr = _
  visit_Expr_TryOperator(
    Self,
    body~ : Expr,
    kind~ : TryOperatorKind,
    try_loc~ : Location,
    loc~ : Location,
  ) -> Expr = _
  visit_Expr_Map(Self, elems~ : @list.List[MapExprElem], loc~ : Location) -> Expr = _
  visit_Expr_Group(Self, expr~ : Expr, group~ : Group, loc~ : Location) -> Expr = _
  visit_Expr_StaticAssert(
    Self,
    asserts~ : @list.List[StaticAssertion],
    body~ : Expr,
  ) -> Expr = _
}

///|
impl MapVisitor with base(self) -> MapVisitorBase[Self] {
  MapVisitorBase::MapVisitorBase(self)
}

///|
impl MapVisitor with visit_Impl(env, impl_) {
  MapVisitorBase::visit_Impl(env, impl_)
}

///|
impl MapVisitor with visit_Impl_TopExpr(
  env,
  expr~,
  is_main~,
  local_types~,
  is_async~,
  loc~,
) {
  MapVisitorBase::visit_Impl_TopExpr(
    env,
    expr~,
    is_main~,
    local_types~,
    is_async~,
    loc~,
  )
}

///|
impl MapVisitor with visit_Impl_TopTest(
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
  MapVisitorBase::visit_Impl_TopTest(
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
impl MapVisitor with visit_Impl_TopTypeDef(env, type_decl) {
  MapVisitorBase::visit_Impl_TopTypeDef(env, type_decl)
}

///|
impl MapVisitor with visit_Impl_TopFuncDef(env, fun_decl~, decl_body~, loc~) {
  MapVisitorBase::visit_Impl_TopFuncDef(env, fun_decl~, decl_body~, loc~)
}

///|
impl MapVisitor with visit_Impl_TopFuncAlias(
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
  MapVisitorBase::visit_Impl_TopFuncAlias(
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
impl MapVisitor with visit_Impl_TopLetDef(
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
  MapVisitorBase::visit_Impl_TopLetDef(
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
impl MapVisitor with visit_Impl_TopTrait(env, trait_decl) {
  MapVisitorBase::visit_Impl_TopTrait(env, trait_decl)
}

///|
impl MapVisitor with visit_Impl_TopBatchTypeAlias(
  env,
  pkg~,
  targets~,
  vis~,
  loc~,
  attrs~,
  is_list~,
  doc~,
) {
  MapVisitorBase::visit_Impl_TopBatchTypeAlias(
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
impl MapVisitor with visit_Impl_TopBatchTraitAlias(
  env,
  pkg~,
  targets~,
  vis~,
  loc~,
  attrs~,
  is_list~,
  doc~,
) {
  MapVisitorBase::visit_Impl_TopBatchTraitAlias(
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
impl MapVisitor with visit_Impl_TopImpl(
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
  MapVisitorBase::visit_Impl_TopImpl(
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
impl MapVisitor with visit_Impl_TopView(
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
  MapVisitorBase::visit_Impl_TopView(
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
impl MapVisitor with visit_Impl_TopImplRelation(
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
  MapVisitorBase::visit_Impl_TopImplRelation(
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
impl MapVisitor with visit_Impl_TopUsing(
  env,
  pkg~,
  names~,
  vis~,
  attrs~,
  loc~,
  doc~,
) {
  MapVisitorBase::visit_Impl_TopUsing(
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
impl MapVisitor with visit_UsingKind(env, using_kind) {
  MapVisitorBase::visit_UsingKind(env, using_kind)
}

///|
impl MapVisitor with visit_Expr(env, expr) {
  MapVisitorBase::visit_Expr(env, expr)
}

///|
impl MapVisitor with visit_Expr_Apply(env, func~, args~, attr~, loc~) {
  MapVisitorBase::visit_Expr_Apply(env, func~, args~, attr~, loc~)
}

///|
impl MapVisitor with visit_Expr_Infix(env, op~, lhs~, rhs~, loc~) {
  MapVisitorBase::visit_Expr_Infix(env, op~, lhs~, rhs~, loc~)
}

///|
impl MapVisitor with visit_Expr_Unary(env, op~, expr~, loc~) {
  MapVisitorBase::visit_Expr_Unary(env, op~, expr~, loc~)
}

///|
impl MapVisitor with visit_Expr_Array(env, exprs~, loc~) {
  MapVisitorBase::visit_Expr_Array(env, exprs~, loc~)
}

///|
impl MapVisitor with visit_Expr_ArraySpread(env, elems~, loc~) {
  MapVisitorBase::visit_Expr_ArraySpread(env, elems~, loc~)
}

///|
impl MapVisitor with visit_Expr_ArrayGet(env, array~, index~, loc~) {
  MapVisitorBase::visit_Expr_ArrayGet(env, array~, index~, loc~)
}

///|
impl MapVisitor with visit_Expr_ArrayGetSlice(
  env,
  array~,
  start_index~,
  end_index~,
  index_loc~,
  loc~,
) {
  MapVisitorBase::visit_Expr_ArrayGetSlice(
    env,
    array~,
    start_index~,
    end_index~,
    index_loc~,
    loc~,
  )
}

///|
impl MapVisitor with visit_Expr_ArraySet(env, array~, index~, value~, loc~) {
  MapVisitorBase::visit_Expr_ArraySet(env, array~, index~, value~, loc~)
}

///|
impl MapVisitor with visit_Expr_ArrayAugmentedSet(
  env,
  op~,
  array~,
  index~,
  value~,
  loc~,
) {
  MapVisitorBase::visit_Expr_ArrayAugmentedSet(
    env,
    op~,
    array~,
    index~,
    value~,
    loc~,
  )
}

///|
impl MapVisitor with visit_Expr_Constant(env, c~, loc~) {
  MapVisitorBase::visit_Expr_Constant(env, c~, loc~)
}

///|
impl MapVisitor with visit_Expr_MultilineString(env, elems~, loc~) {
  MapVisitorBase::visit_Expr_MultilineString(env, elems~, loc~)
}

///|
impl MapVisitor with visit_Expr_Interp(env, elems~, loc~) {
  MapVisitorBase::visit_Expr_Interp(env, elems~, loc~)
}

///|
impl MapVisitor with visit_Expr_Constraint(env, expr~, ty~, loc~) {
  MapVisitorBase::visit_Expr_Constraint(env, expr~, ty~, loc~)
}

///|
impl MapVisitor with visit_Expr_Constr(env, constr~, loc~) {
  MapVisitorBase::visit_Expr_Constr(env, constr~, loc~)
}

///|
impl MapVisitor with visit_Expr_While(
  env,
  loop_cond~,
  loop_body~,
  while_else~,
  label~,
  loc~,
) {
  MapVisitorBase::visit_Expr_While(
    env,
    loop_cond~,
    loop_body~,
    while_else~,
    label~,
    loc~,
  )
}

///|
impl MapVisitor with visit_Expr_Function(env, func~, loc~) {
  MapVisitorBase::visit_Expr_Function(env, func~, loc~)
}

///|
impl MapVisitor with visit_Expr_Ident(env, id~, loc~) {
  MapVisitorBase::visit_Expr_Ident(env, id~, loc~)
}

///|
impl MapVisitor with visit_Expr_If(env, cond~, ifso~, ifnot~, loc~) {
  MapVisitorBase::visit_Expr_If(env, cond~, ifso~, ifnot~, loc~)
}

///|
impl MapVisitor with visit_Expr_Guard(env, cond~, otherwise~, body~, loc~) {
  MapVisitorBase::visit_Expr_Guard(env, cond~, otherwise~, body~, loc~)
}

///|
impl MapVisitor with visit_Expr_Is(env, expr~, pat~, loc~) {
  MapVisitorBase::visit_Expr_Is(env, expr~, pat~, loc~)
}

///|
impl MapVisitor with visit_Expr_Defer(env, expr~, body~, loc~) {
  MapVisitorBase::visit_Expr_Defer(env, expr~, body~, loc~)
}

///|
impl MapVisitor with visit_Expr_LetFn(env, name~, func~, body~, loc~) {
  MapVisitorBase::visit_Expr_LetFn(env, name~, func~, body~, loc~)
}

///|
impl MapVisitor with visit_Expr_LetRec(env, bindings~, body~, loc~) {
  MapVisitorBase::visit_Expr_LetRec(env, bindings~, body~, loc~)
}

///|
impl MapVisitor with visit_Expr_LetAnd(env, bindings~, body~, loc~) {
  MapVisitorBase::visit_Expr_LetAnd(env, bindings~, body~, loc~)
}

///|
impl MapVisitor with visit_Expr_Let(env, pattern~, expr~, body~, loc~) {
  MapVisitorBase::visit_Expr_Let(env, pattern~, expr~, body~, loc~)
}

///|
impl MapVisitor with visit_Expr_Sequence(env, exprs~, last_expr~, loc~) {
  MapVisitorBase::visit_Expr_Sequence(env, exprs~, last_expr~, loc~)
}

///|
impl MapVisitor with visit_Expr_Tuple(env, exprs~, loc~) {
  MapVisitorBase::visit_Expr_Tuple(env, exprs~, loc~)
}

///|
impl MapVisitor with visit_Expr_Record(
  env,
  type_name~,
  fields~,
  trailing~,
  loc~,
) {
  MapVisitorBase::visit_Expr_Record(env, type_name~, fields~, trailing~, loc~)
}

///|
impl MapVisitor with visit_Expr_RecordUpdate(
  env,
  type_name~,
  record~,
  fields~,
  loc~,
) {
  MapVisitorBase::visit_Expr_RecordUpdate(
    env,
    type_name~,
    record~,
    fields~,
    loc~,
  )
}

///|
impl MapVisitor with visit_Expr_Field(env, record~, accessor~, loc~) {
  MapVisitorBase::visit_Expr_Field(env, record~, accessor~, loc~)
}

///|
impl MapVisitor with visit_Expr_Method(env, type_name~, method_name~, loc~) {
  MapVisitorBase::visit_Expr_Method(env, type_name~, method_name~, loc~)
}

///|
impl MapVisitor with visit_Expr_DotApply(
  env,
  self~,
  method_name~,
  args~,
  return_self~,
  attr~,
  loc~,
) {
  MapVisitorBase::visit_Expr_DotApply(
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
impl MapVisitor with visit_Expr_As(env, expr~, trait_~, loc~) {
  MapVisitorBase::visit_Expr_As(env, expr~, trait_~, loc~)
}

///|
impl MapVisitor with visit_Expr_Mutate(
  env,
  record~,
  accessor~,
  field~,
  augmented_by~,
  loc~,
) {
  MapVisitorBase::visit_Expr_Mutate(
    env,
    record~,
    accessor~,
    field~,
    augmented_by~,
    loc~,
  )
}

///|
impl MapVisitor with visit_Expr_Match(env, expr~, cases~, match_loc~, loc~) {
  MapVisitorBase::visit_Expr_Match(env, expr~, cases~, match_loc~, loc~)
}

///|
impl MapVisitor with visit_Expr_LetMut(env, binder~, ty~, expr~, body~, loc~) {
  MapVisitorBase::visit_Expr_LetMut(env, binder~, ty~, expr~, body~, loc~)
}

///|
impl MapVisitor with visit_Expr_Pipe(env, lhs~, rhs~, loc~) {
  MapVisitorBase::visit_Expr_Pipe(env, lhs~, rhs~, loc~)
}

///|
impl MapVisitor with visit_Expr_Assign(env, var_~, expr~, augmented_by~, loc~) {
  MapVisitorBase::visit_Expr_Assign(env, var_~, expr~, augmented_by~, loc~)
}

///|
impl MapVisitor with visit_Expr_Hole(env, loc~, kind~) {
  MapVisitorBase::visit_Expr_Hole(env, loc~, kind~)
}

///|
impl MapVisitor with visit_Expr_Return(env, return_value~, loc~) {
  MapVisitorBase::visit_Expr_Return(env, return_value~, loc~)
}

///|
impl MapVisitor with visit_Expr_Raise(env, err_value~, loc~) {
  MapVisitorBase::visit_Expr_Raise(env, err_value~, loc~)
}

///|
impl MapVisitor with visit_Expr_Unit(env, loc~, faked~) {
  MapVisitorBase::visit_Expr_Unit(env, loc~, faked~)
}

///|
impl MapVisitor with visit_Expr_Break(env, arg~, label~, loc~) {
  MapVisitorBase::visit_Expr_Break(env, arg~, label~, loc~)
}

///|
impl MapVisitor with visit_Expr_Continue(env, args~, label~, loc~) {
  MapVisitorBase::visit_Expr_Continue(env, args~, label~, loc~)
}

///|
impl MapVisitor with visit_Expr_Loop(env, arg~, body~, label~, loop_loc~, loc~) {
  MapVisitorBase::visit_Expr_Loop(env, arg~, body~, label~, loop_loc~, loc~)
}

///|
impl MapVisitor with visit_Expr_For(
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
  MapVisitorBase::visit_Expr_For(
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
impl MapVisitor with visit_Expr_ForEach(
  env,
  binders~,
  expr~,
  body~,
  else_block~,
  label~,
  loc~,
) {
  MapVisitorBase::visit_Expr_ForEach(
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
impl MapVisitor with visit_Expr_Try(
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
  MapVisitorBase::visit_Expr_Try(
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
impl MapVisitor with visit_Expr_TryOperator(env, body~, kind~, try_loc~, loc~) {
  MapVisitorBase::visit_Expr_TryOperator(env, body~, kind~, try_loc~, loc~)
}

///|
impl MapVisitor with visit_Expr_Map(env, elems~, loc~) {
  MapVisitorBase::visit_Expr_Map(env, elems~, loc~)
}

///|
impl MapVisitor with visit_Expr_Group(env, expr~, group~, loc~) {
  MapVisitorBase::visit_Expr_Group(env, expr~, group~, loc~)
}

///|
impl MapVisitor with visit_Expr_StaticAssert(env, asserts~, body~) {
  MapVisitorBase::visit_Expr_StaticAssert(env, asserts~, body~)
}

///|
impl MapVisitor with visit_Visibility(env, visibility) {
  MapVisitorBase::visit_Visibility(env, visibility)
}

///|
impl MapVisitor with visit_Visibility_Pub(env, attr~, loc~) {
  MapVisitorBase::visit_Visibility_Pub(env, attr~, loc~)
}

///|
impl MapVisitor with visit_Visibility_Priv(env, loc~) {
  MapVisitorBase::visit_Visibility_Priv(env, loc~)
}

///|
impl MapVisitor with visit_Constant(env, constant) {
  MapVisitorBase::visit_Constant(env, constant)
}

///|
impl MapVisitor with visit_Label(env, label) {
  MapVisitorBase::visit_Label(env, label)
}

///|
impl MapVisitor with visit_ConstrName(env, constr_name) {
  MapVisitorBase::visit_ConstrName(env, constr_name)
}

///|
impl MapVisitor with visit_LongIdent(env, long_ident) {
  MapVisitorBase::visit_LongIdent(env, long_ident)
}

///|
impl MapVisitor with visit_LongIdent_Ident(env, name~) {
  MapVisitorBase::visit_LongIdent_Ident(env, name~)
}

///|
impl MapVisitor with visit_LongIdent_Dot(env, pkg~, id~) {
  MapVisitorBase::visit_LongIdent_Dot(env, pkg~, id~)
}

///|
impl MapVisitor with visit_TypeName(env, type_name) {
  MapVisitorBase::visit_TypeName(env, type_name)
}

///|
impl MapVisitor with visit_ConstrId(env, constr_id) {
  MapVisitorBase::visit_ConstrId(env, constr_id)
}

///|
impl MapVisitor with visit_Type(env, type_) {
  MapVisitorBase::visit_Type(env, type_)
}

///|
impl MapVisitor with visit_Type_Any(env, loc~) {
  MapVisitorBase::visit_Type_Any(env, loc~)
}

///|
impl MapVisitor with visit_Type_Arrow(env, args~, res~, err~, is_async~, loc~) {
  MapVisitorBase::visit_Type_Arrow(env, args~, res~, err~, is_async~, loc~)
}

///|
impl MapVisitor with visit_Type_Tuple(env, tys~, loc~) {
  MapVisitorBase::visit_Type_Tuple(env, tys~, loc~)
}

///|
impl MapVisitor with visit_Type_Name(env, constr_id~, tys~, loc~) {
  MapVisitorBase::visit_Type_Name(env, constr_id~, tys~, loc~)
}

///|
impl MapVisitor with visit_Type_Option(env, ty~, loc~, question_loc~) {
  MapVisitorBase::visit_Type_Option(env, ty~, loc~, question_loc~)
}

///|
impl MapVisitor with visit_Type_Object(env, constr_id) {
  MapVisitorBase::visit_Type_Object(env, constr_id)
}

///|
impl MapVisitor with visit_ErrorType(env, error_type) {
  MapVisitorBase::visit_ErrorType(env, error_type)
}

///|
impl MapVisitor with visit_ErrorType_ErrorType(env, ty~) {
  MapVisitorBase::visit_ErrorType_ErrorType(env, ty~)
}

///|
impl MapVisitor with visit_ErrorType_DefaultErrorType(env, loc~) {
  MapVisitorBase::visit_ErrorType_DefaultErrorType(env, loc~)
}

///|
impl MapVisitor with visit_ErrorType_Noraise(env, loc~) {
  MapVisitorBase::visit_ErrorType_Noraise(env, loc~)
}

///|
impl MapVisitor with visit_ErrorType_MaybeError(env, ty~) {
  MapVisitorBase::visit_ErrorType_MaybeError(env, ty~)
}

///|
impl MapVisitor with visit_ConstrParam(env, constr_param) {
  MapVisitorBase::visit_ConstrParam(env, constr_param)
}

///|
impl MapVisitor with visit_ConstrDecl(env, constr_decl) {
  MapVisitorBase::visit_ConstrDecl(env, constr_decl)
}

///|
impl MapVisitor with visit_ExceptionDecl(env, exception_decl) {
  MapVisitorBase::visit_ExceptionDecl(env, exception_decl)
}

///|
impl MapVisitor with visit_ExceptionDecl_SinglePayload(env, type_) {
  MapVisitorBase::visit_ExceptionDecl_SinglePayload(env, type_)
}

///|
impl MapVisitor with visit_ExceptionDecl_EnumPayload(env, constr_decls) {
  MapVisitorBase::visit_ExceptionDecl_EnumPayload(env, constr_decls)
}

///|
impl MapVisitor with visit_FieldName(env, field_name) {
  MapVisitorBase::visit_FieldName(env, field_name)
}

///|
impl MapVisitor with visit_FieldDecl(env, field_decl) {
  MapVisitorBase::visit_FieldDecl(env, field_decl)
}

///|
impl MapVisitor with visit_TypeDesc(env, type_desc) {
  MapVisitorBase::visit_TypeDesc(env, type_desc)
}

///|
impl MapVisitor with visit_TypeDesc_Newtype(env, type_) {
  MapVisitorBase::visit_TypeDesc_Newtype(env, type_)
}

///|
impl MapVisitor with visit_TypeDesc_Error(env, exception_decl) {
  MapVisitorBase::visit_TypeDesc_Error(env, exception_decl)
}

///|
impl MapVisitor with visit_TypeDesc_Variant(env, constr_decls) {
  MapVisitorBase::visit_TypeDesc_Variant(env, constr_decls)
}

///|
impl MapVisitor with visit_TypeDesc_Record(env, field_decls) {
  MapVisitorBase::visit_TypeDesc_Record(env, field_decls)
}

///|
impl MapVisitor with visit_TypeDesc_TupleStruct(env, types) {
  MapVisitorBase::visit_TypeDesc_TupleStruct(env, types)
}

///|
impl MapVisitor with visit_TypeDesc_Alias(env, type_) {
  MapVisitorBase::visit_TypeDesc_Alias(env, type_)
}

///|
impl MapVisitor with visit_Hole(env, hole) {
  MapVisitorBase::visit_Hole(env, hole)
}

///|
impl MapVisitor with visit_ArgumentKind(env, argument_kind) {
  MapVisitorBase::visit_ArgumentKind(env, argument_kind)
}

///|
impl MapVisitor with visit_ArgumentKind_Labelled(env, label) {
  MapVisitorBase::visit_ArgumentKind_Labelled(env, label)
}

///|
impl MapVisitor with visit_ArgumentKind_LabelledPun(env, label) {
  MapVisitorBase::visit_ArgumentKind_LabelledPun(env, label)
}

///|
impl MapVisitor with visit_ArgumentKind_LabelledOption(
  env,
  label~,
  question_loc~,
) {
  MapVisitorBase::visit_ArgumentKind_LabelledOption(env, label~, question_loc~)
}

///|
impl MapVisitor with visit_ArgumentKind_LabelledOptionPun(
  env,
  label~,
  question_loc~,
) {
  MapVisitorBase::visit_ArgumentKind_LabelledOptionPun(
    env,
    label~,
    question_loc~,
  )
}

///|
impl MapVisitor with visit_FnKind(env, fn_kind) {
  MapVisitorBase::visit_FnKind(env, fn_kind)
}

///|
impl MapVisitor with visit_Group(env, group) {
  MapVisitorBase::visit_Group(env, group)
}

///|
impl MapVisitor with visit_TrailingMark(env, trailing_mark) {
  MapVisitorBase::visit_TrailingMark(env, trailing_mark)
}

///|
impl MapVisitor with visit_ApplyAttr(env, apply_attr) {
  MapVisitorBase::visit_ApplyAttr(env, apply_attr)
}

///|
impl MapVisitor with visit_TypeVarConstraint(env, type_var_constraint) {
  MapVisitorBase::visit_TypeVarConstraint(env, type_var_constraint)
}

///|
impl MapVisitor with visit_TypeVarBinder(env, type_var_binder) {
  MapVisitorBase::visit_TypeVarBinder(env, type_var_binder)
}

///|
impl MapVisitor with visit_TypeDeclBinder(env, type_decl_binder) {
  MapVisitorBase::visit_TypeDeclBinder(env, type_decl_binder)
}

///|
impl MapVisitor with visit_Binder(env, binder) {
  MapVisitorBase::visit_Binder(env, binder)
}

///|
impl MapVisitor with visit_Var(env, var_) {
  MapVisitorBase::visit_Var(env, var_)
}

///|
impl MapVisitor with visit_ConstructorExtraInfo(env, constructor_extra_info) {
  MapVisitorBase::visit_ConstructorExtraInfo(env, constructor_extra_info)
}

///|
impl MapVisitor with visit_ConstructorExtraInfo_TypeName(env, type_name) {
  MapVisitorBase::visit_ConstructorExtraInfo_TypeName(env, type_name)
}

///|
impl MapVisitor with visit_ConstructorExtraInfo_Package(env, package_) {
  MapVisitorBase::visit_ConstructorExtraInfo_Package(env, package_)
}

///|
impl MapVisitor with visit_Constructor(env, constructor_) {
  MapVisitorBase::visit_Constructor(env, constructor_)
}

///|
impl MapVisitor with visit_Accessor(env, accessor) {
  MapVisitorBase::visit_Accessor(env, accessor)
}

///|
impl MapVisitor with visit_Accessor_Label(env, label) {
  MapVisitorBase::visit_Accessor_Label(env, label)
}

///|
impl MapVisitor with visit_Accessor_Index(env, tuple_index~, loc~) {
  MapVisitorBase::visit_Accessor_Index(env, tuple_index~, loc~)
}

///|
impl MapVisitor with visit_Accessor_Newtype(env, loc~) {
  MapVisitorBase::visit_Accessor_Newtype(env, loc~)
}

///|
impl MapVisitor with visit_AliasTarget(env, alias_target) {
  MapVisitorBase::visit_AliasTarget(env, alias_target)
}

///|
impl MapVisitor with visit_Argument(env, argument) {
  MapVisitorBase::visit_Argument(env, argument)
}

///|
impl MapVisitor with visit_Parameter(env, parameter) {
  MapVisitorBase::visit_Parameter(env, parameter)
}

///|
impl MapVisitor with visit_Parameter_DiscardPositional(env, ty~, loc~) {
  MapVisitorBase::visit_Parameter_DiscardPositional(env, ty~, loc~)
}

///|
impl MapVisitor with visit_Parameter_Positional(env, binder~, ty~) {
  MapVisitorBase::visit_Parameter_Positional(env, binder~, ty~)
}

///|
impl MapVisitor with visit_Parameter_Labelled(env, binder~, ty~) {
  MapVisitorBase::visit_Parameter_Labelled(env, binder~, ty~)
}

///|
impl MapVisitor with visit_Parameter_Optional(env, binder~, default~, ty~) {
  MapVisitorBase::visit_Parameter_Optional(env, binder~, default~, ty~)
}

///|
impl MapVisitor with visit_Parameter_QuestionOptional(env, binder~, ty~) {
  MapVisitorBase::visit_Parameter_QuestionOptional(env, binder~, ty~)
}

///|
impl MapVisitor with visit_Case(env, case) {
  MapVisitorBase::visit_Case(env, case)
}

///|
impl MapVisitor with visit_SpreadableElem(env, spreadable_elem) {
  MapVisitorBase::visit_SpreadableElem(env, spreadable_elem)
}

///|
impl MapVisitor with visit_SpreadableElem_Regular(env, expr) {
  MapVisitorBase::visit_SpreadableElem_Regular(env, expr)
}

///|
impl MapVisitor with visit_SpreadableElem_Spread(env, expr~, loc~) {
  MapVisitorBase::visit_SpreadableElem_Spread(env, expr~, loc~)
}

///|
impl MapVisitor with visit_MapExprElem(env, map_expr_elem) {
  MapVisitorBase::visit_MapExprElem(env, map_expr_elem)
}

///|
impl MapVisitor with visit_StaticAssertion(env, static_assertion) {
  MapVisitorBase::visit_StaticAssertion(env, static_assertion)
}

///|
impl MapVisitor with visit_Func(env, func) {
  MapVisitorBase::visit_Func(env, func)
}

///|
impl MapVisitor with visit_FieldDef(env, field_def) {
  MapVisitorBase::visit_FieldDef(env, field_def)
}

///|
impl MapVisitor with visit_InterpElem(env, interp_elem) {
  MapVisitorBase::visit_InterpElem(env, interp_elem)
}

///|
impl MapVisitor with visit_InterpElem_Literal(env, repr~, loc~) {
  MapVisitorBase::visit_InterpElem_Literal(env, repr~, loc~)
}

///|
impl MapVisitor with visit_InterpElem_Expr(env, expr~, loc~) {
  MapVisitorBase::visit_InterpElem_Expr(env, expr~, loc~)
}

///|
impl MapVisitor with visit_InterpElem_Source(env, interp_source) {
  MapVisitorBase::visit_InterpElem_Source(env, interp_source)
}

///|
impl MapVisitor with visit_MultilineStringElem(env, multiline_string_elem) {
  MapVisitorBase::visit_MultilineStringElem(env, multiline_string_elem)
}

///|
impl MapVisitor with visit_MultilineStringElem_String(env, string_val) {
  MapVisitorBase::visit_MultilineStringElem_String(env, string_val)
}

///|
impl MapVisitor with visit_MultilineStringElem_Interp(env, interp_elems) {
  MapVisitorBase::visit_MultilineStringElem_Interp(env, interp_elems)
}

///|
impl MapVisitor with visit_TryOperatorKind(env, try_operator_kind) {
  MapVisitorBase::visit_TryOperatorKind(env, try_operator_kind)
}

///|
impl MapVisitor with visit_DotDotBinder(env, dot_dot_binder) {
  MapVisitorBase::visit_DotDotBinder(env, dot_dot_binder)
}

///|
impl MapVisitor with visit_DotDotBinder_BinderAs(env, binder) {
  MapVisitorBase::visit_DotDotBinder_BinderAs(env, binder)
}

///|
impl MapVisitor with visit_DotDotBinder_Binder(env, binder) {
  MapVisitorBase::visit_DotDotBinder_Binder(env, binder)
}

///|
impl MapVisitor with visit_ArrayPattern(env, array_pattern) {
  MapVisitorBase::visit_ArrayPattern(env, array_pattern)
}

///|
impl MapVisitor with visit_ArrayPattern_Pattern(env, pattern) {
  MapVisitorBase::visit_ArrayPattern_Pattern(env, pattern)
}

///|
impl MapVisitor with visit_ArrayPattern_StringSpread(env, str~, loc~) {
  MapVisitorBase::visit_ArrayPattern_StringSpread(env, str~, loc~)
}

///|
impl MapVisitor with visit_ArrayPattern_BytesSpread(env, bytes~, loc~) {
  MapVisitorBase::visit_ArrayPattern_BytesSpread(env, bytes~, loc~)
}

///|
impl MapVisitor with visit_ArrayPattern_ConstSpread(env, binder~, pkg~, loc~) {
  MapVisitorBase::visit_ArrayPattern_ConstSpread(env, binder~, pkg~, loc~)
}

///|
impl MapVisitor with visit_ArrayPatterns(env, array_patterns) {
  MapVisitorBase::visit_ArrayPatterns(env, array_patterns)
}

///|
impl MapVisitor with visit_ArrayPatterns_Closed(env, array_patterns) {
  MapVisitorBase::visit_ArrayPatterns_Closed(env, array_patterns)
}

///|
impl MapVisitor with visit_ArrayPatterns_Open(
  env,
  prefix_patterns,
  suffix_patterns,
  dot_dot_binder,
) {
  MapVisitorBase::visit_ArrayPatterns_Open(
    env, prefix_patterns, suffix_patterns, dot_dot_binder,
  )
}

///|
impl MapVisitor with visit_WhereClause(env, where_clause) {
  MapVisitorBase::visit_WhereClause(env, where_clause)
}

///|
impl MapVisitor with visit_FieldPat(env, field_pat) {
  MapVisitorBase::visit_FieldPat(env, field_pat)
}

///|
impl MapVisitor with visit_ConstrPatArg(env, const_pat_arg) {
  MapVisitorBase::visit_ConstrPatArg(env, const_pat_arg)
}

///|
impl MapVisitor with visit_MapPatElem(env, map_pat_elem) {
  MapVisitorBase::visit_MapPatElem(env, map_pat_elem)
}

///|
impl MapVisitor with visit_RangeKind(env, kind) {
  MapVisitorBase::visit_RangeKind(env, kind)
}

///|
impl MapVisitor with visit_Pattern(env, pattern) {
  MapVisitorBase::visit_Pattern(env, pattern)
}

///|
impl MapVisitor with visit_Pattern_Alias(env, pat~, alias_~, loc~) {
  MapVisitorBase::visit_Pattern_Alias(env, pat~, alias_~, loc~)
}

///|
impl MapVisitor with visit_Pattern_Any(env, loc~) {
  MapVisitorBase::visit_Pattern_Any(env, loc~)
}

///|
impl MapVisitor with visit_Pattern_Array(env, pats~, loc~) {
  MapVisitorBase::visit_Pattern_Array(env, pats~, loc~)
}

///|
impl MapVisitor with visit_Pattern_Constant(env, c~, loc~) {
  MapVisitorBase::visit_Pattern_Constant(env, c~, loc~)
}

///|
impl MapVisitor with visit_Pattern_Constraint(env, pat~, ty~, loc~) {
  MapVisitorBase::visit_Pattern_Constraint(env, pat~, ty~, loc~)
}

///|
impl MapVisitor with visit_Pattern_Constr(env, constr~, args~, is_open~, loc~) {
  MapVisitorBase::visit_Pattern_Constr(env, constr~, args~, is_open~, loc~)
}

///|
impl MapVisitor with visit_Pattern_Or(env, pat1~, pat2~, loc~) {
  MapVisitorBase::visit_Pattern_Or(env, pat1~, pat2~, loc~)
}

///|
impl MapVisitor with visit_Pattern_Tuple(env, pats~, loc~) {
  MapVisitorBase::visit_Pattern_Tuple(env, pats~, loc~)
}

///|
impl MapVisitor with visit_Pattern_Var(env, binder) {
  MapVisitorBase::visit_Pattern_Var(env, binder)
}

///|
impl MapVisitor with visit_Pattern_Record(env, fields~, is_closed~, loc~) {
  MapVisitorBase::visit_Pattern_Record(env, fields~, is_closed~, loc~)
}

///|
impl MapVisitor with visit_Pattern_Map(env, elems~, is_closed~, loc~) {
  MapVisitorBase::visit_Pattern_Map(env, elems~, is_closed~, loc~)
}

///|
impl MapVisitor with visit_Pattern_Range(env, lhs~, rhs~, kind~, loc~) {
  MapVisitorBase::visit_Pattern_Range(env, lhs~, rhs~, kind~, loc~)
}

///|
impl MapVisitor with visit_Pattern_SpecialConstr(env, binder~, args~, loc~) {
  MapVisitorBase::visit_Pattern_SpecialConstr(env, binder~, args~, loc~)
}

///|
impl MapVisitor with visit_LocalTypeDecl(env, localtype_decl) {
  MapVisitorBase::visit_LocalTypeDecl(env, localtype_decl)
}

///|
impl MapVisitor with visit_DerivingDirective(env, deriving_directive) {
  MapVisitorBase::visit_DerivingDirective(env, deriving_directive)
}

///|
impl MapVisitor with visit_TypeDecl(env, type_decl) {
  MapVisitorBase::visit_TypeDecl(env, type_decl)
}

///|
impl MapVisitor with visit_FuncStubs(env, func_stubs) {
  MapVisitorBase::visit_FuncStubs(env, func_stubs)
}

///|
impl MapVisitor with visit_FuncStubs_Import(
  env,
  module_name~,
  func_name~,
  language~,
) {
  MapVisitorBase::visit_FuncStubs_Import(
    env,
    module_name~,
    func_name~,
    language~,
  )
}

///|
impl MapVisitor with visit_FuncStubs_Embedded(env, language~, code~) {
  MapVisitorBase::visit_FuncStubs_Embedded(env, language~, code~)
}

///|
impl MapVisitor with visit_EmbeddedCode(env, embedded_code) {
  MapVisitorBase::visit_EmbeddedCode(env, embedded_code)
}

///|
impl MapVisitor with visit_EmbeddedCode_CodeString(env, string_literal) {
  MapVisitorBase::visit_EmbeddedCode_CodeString(env, string_literal)
}

///|
impl MapVisitor with visit_EmbeddedCode_CodeMultilineString(env, strings) {
  MapVisitorBase::visit_EmbeddedCode_CodeMultilineString(env, strings)
}

///|
impl MapVisitor with visit_DeclBody(env, decl_body) {
  MapVisitorBase::visit_DeclBody(env, decl_body)
}

///|
impl MapVisitor with visit_DeclBody_DeclBody(env, local_types~, expr~) {
  MapVisitorBase::visit_DeclBody_DeclBody(env, local_types~, expr~)
}

///|
impl MapVisitor with visit_DeclBody_DeclStubs(env, func_stubs) {
  MapVisitorBase::visit_DeclBody_DeclStubs(env, func_stubs)
}

///|
impl MapVisitor with visit_FunDecl(env, fun_decl) {
  MapVisitorBase::visit_FunDecl(env, fun_decl)
}

///|
impl MapVisitor with visit_TraitMethodDecl(env, trait_method_decl) {
  MapVisitorBase::visit_TraitMethodDecl(env, trait_method_decl)
}

///|
impl MapVisitor with visit_TraitDecl(env, trait_decl) {
  MapVisitorBase::visit_TraitDecl(env, trait_decl)
}

///|
impl MapVisitor with visit_Expr_IsLexMatch(
  env,
  expr~ : Expr,
  strategy~,
  pat~,
  pat_loc~,
  loc~,
) {
  MapVisitorBase::visit_Expr_IsLexMatch(
    env,
    expr~,
    strategy~,
    pat~,
    pat_loc~,
    loc~,
  )
}

///|
impl MapVisitor with visit_Expr_LexMatch(
  env,
  strategy~,
  expr~,
  match_loc~,
  cases~,
  loc~,
) {
  MapVisitorBase::visit_Expr_LexMatch(
    env,
    strategy~,
    expr~,
    match_loc~,
    cases~,
    loc~,
  )
}

///|
impl MapVisitor with visit_LexCase(env, lex_case) {
  MapVisitorBase::visit_LexCase(env, lex_case)
}

///|
impl MapVisitor with visit_LexTopPattern(env, lex_top_pattern) {
  MapVisitorBase::visit_LexTopPattern(env, lex_top_pattern)
}

///|
impl MapVisitor with visit_LexTopPattern_Pattern(env, lex_pattern) {
  MapVisitorBase::visit_LexTopPattern_Pattern(env, lex_pattern)
}

///|
impl MapVisitor with visit_LexTopPattern_Binder(env, binder) {
  MapVisitorBase::visit_LexTopPattern_Binder(env, binder)
}

///|
impl MapVisitor with visit_LexTopPattern_Wildcard(env, loc~) {
  MapVisitorBase::visit_LexTopPattern_Wildcard(env, loc~)
}

///|
impl MapVisitor with visit_LexPattern(env, lex_pattern) {
  MapVisitorBase::visit_LexPattern(env, lex_pattern)
}

///|
impl MapVisitor with visit_LexPattern_Regex(env, lit~, loc~) {
  MapVisitorBase::visit_LexPattern_Regex(env, lit~, loc~)
}

///|
impl MapVisitor with visit_LexPattern_RegexInterp(env, elems~, loc~) {
  MapVisitorBase::visit_LexPattern_RegexInterp(env, elems~, loc~)
}

///|
impl MapVisitor with visit_LexPattern_Alias(env, pat~, binder~, loc~) {
  MapVisitorBase::visit_LexPattern_Alias(env, pat~, binder~, loc~)
}

///|
impl MapVisitor with visit_LexPattern_Sequence(env, pats~, loc~) {
  MapVisitorBase::visit_LexPattern_Sequence(env, pats~, loc~)
}

///|
pub(all) struct MapVisitorBase[T](T)

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Impl(
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
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Visibility(
  env,
  visibility,
) {
  match visibility {
    Default => Default
    Pub(attr~, loc~) => env.0.visit_Visibility_Pub(attr~, loc~)
    Priv(loc~) => env.0.visit_Visibility_Priv(loc~)
  }
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Visibility_Pub(
  _,
  attr~,
  loc~,
) {
  Pub(attr~, loc~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Visibility_Priv(
  _,
  loc~,
) {
  Priv(loc~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Constant(
  _,
  constant,
) {
  constant
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Label(
  _,
  label,
) {
  label
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_ConstrName(
  _,
  constr_name,
) {
  constr_name
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_LongIdent(
  env,
  long_ident,
) {
  match long_ident {
    Ident(name~) => env.0.visit_LongIdent_Ident(name~)
    Dot(pkg~, id~) => env.0.visit_LongIdent_Dot(pkg~, id~)
  }
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_LongIdent_Ident(
  _,
  name~,
) {
  Ident(name~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_LongIdent_Dot(
  _,
  pkg~,
  id~,
) {
  Dot(pkg~, id~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_TypeName(
  env,
  type_name,
) {
  let name = env.0.visit_LongIdent(type_name.name)
  let is_object = type_name.is_object
  let loc = type_name.loc
  TypeName::{ name, is_object, loc }
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_ConstrId(
  env,
  constr_id,
) {
  let id = env.0.visit_LongIdent(constr_id.id)
  let loc = constr_id.loc
  ConstrId::{ id, loc }
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Type(
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
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Type_Any(
  _,
  loc~,
) {
  Any(loc~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Type_Arrow(
  env,
  args~,
  res~,
  err~,
  is_async~,
  loc~,
) {
  let args = args.map(x => env.0.visit_Type(x))
  let res = env.0.visit_Type(res)
  let err = env.0.visit_ErrorType(err)
  Arrow(args~, res~, err~, is_async~, loc~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Type_Tuple(
  env,
  tys~,
  loc~,
) {
  let tys = tys.map(x => env.0.visit_Type(x))
  Tuple(tys~, loc~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Type_Name(
  env,
  constr_id~,
  tys~,
  loc~,
) {
  let constr_id = env.0.visit_ConstrId(constr_id)
  let tys = tys.map(x => env.0.visit_Type(x))
  Name(constr_id~, tys~, loc~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Type_Option(
  env,
  ty~,
  loc~,
  question_loc~,
) {
  let ty = env.0.visit_Type(ty)
  Option(ty~, loc~, question_loc~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Type_Object(
  env,
  constr_id,
) {
  let constr_id = env.0.visit_ConstrId(constr_id)
  Object(constr_id)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_ErrorType(
  env,
  error_type,
) {
  match error_type {
    ErrorType(ty~) => env.0.visit_ErrorType_ErrorType(ty~)
    DefaultErrorType(loc~) => env.0.visit_ErrorType_DefaultErrorType(loc~)
    NoErrorType => NoErrorType
    Noraise(loc~) => env.0.visit_ErrorType_Noraise(loc~)
    MaybeError(ty~) => env.0.visit_ErrorType_MaybeError(ty~)
  }
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_ErrorType_ErrorType(
  env,
  ty~,
) {
  let ty = env.0.visit_Type(ty)
  ErrorType(ty~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_ErrorType_DefaultErrorType(
  _,
  loc~,
) {
  DefaultErrorType(loc~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_ErrorType_Noraise(
  _,
  loc~,
) {
  Noraise(loc~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_ErrorType_MaybeError(
  env,
  ty~,
) {
  let ty = env.0.visit_Type(ty)
  MaybeError(ty~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_ConstrParam(
  env,
  constr_param,
) {
  let ty = env.0.visit_Type(constr_param.ty)
  let label = constr_param.label.map(x => env.0.visit_Label(x))
  let mut_ = constr_param.mut_
  ConstrParam::{ ty, mut_, label }
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_ConstrDecl(
  env,
  constr_decl,
) {
  let name = env.0.visit_ConstrName(constr_decl.name)
  let args = constr_decl.args.map(args => {
    args.map(x => env.0.visit_ConstrParam(x))
  })
  let tag = constr_decl.tag
  let loc = constr_decl.loc
  let attrs = constr_decl.attrs
  let doc = constr_decl.doc
  ConstrDecl::{ name, args, tag, loc, attrs, doc }
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_ExceptionDecl(
  env,
  exception_decl,
) {
  match exception_decl {
    NoPayload => NoPayload
    SinglePayload(type_) => env.0.visit_ExceptionDecl_SinglePayload(type_)
    EnumPayload(constr_decls) =>
      env.0.visit_ExceptionDecl_EnumPayload(constr_decls)
  }
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_ExceptionDecl_SinglePayload(
  env,
  type_,
) {
  let type_ = env.0.visit_Type(type_)
  SinglePayload(type_)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_ExceptionDecl_EnumPayload(
  env,
  constr_decls,
) {
  let constr_decls = constr_decls.map(x => env.0.visit_ConstrDecl(x))
  EnumPayload(constr_decls)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_FieldName(
  _,
  fieldname,
) {
  fieldname
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_FieldDecl(
  env,
  field_decl,
) {
  let name = env.0.visit_FieldName(field_decl.name)
  let ty = env.0.visit_Type(field_decl.ty)
  let vis = env.0.visit_Visibility(field_decl.vis)
  let mut_ = field_decl.mut_
  let loc = field_decl.loc
  let attrs = field_decl.attrs
  let doc = field_decl.doc
  FieldDecl::{ name, ty, mut_, vis, loc, attrs, doc }
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_TypeDesc(
  env,
  type_desc,
) {
  match type_desc {
    Abstract => Abstract
    Extern => Extern
    Newtype(type_) => env.0.visit_TypeDesc_Newtype(type_)
    Error(exception_decl) => env.0.visit_TypeDesc_Error(exception_decl)
    Variant(constr_decls) => env.0.visit_TypeDesc_Variant(constr_decls)
    Record(field_decls) => env.0.visit_TypeDesc_Record(field_decls)
    TupleStruct(types) => env.0.visit_TypeDesc_TupleStruct(types)
    Alias(type_) => env.0.visit_TypeDesc_Alias(type_)
  }
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_TypeDesc_Newtype(
  env,
  type_,
) {
  let type_ = env.0.visit_Type(type_)
  Newtype(type_)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_TypeDesc_Error(
  env,
  exception_decl,
) {
  let exception_decl = env.0.visit_ExceptionDecl(exception_decl)
  Error(exception_decl)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_TypeDesc_Variant(
  env,
  constr_decls,
) {
  let constr_decls = constr_decls.map(x => env.0.visit_ConstrDecl(x))
  Variant(constr_decls)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_TypeDesc_Record(
  env,
  field_decls,
) {
  let field_decls = field_decls.map(x => env.0.visit_FieldDecl(x))
  Record(field_decls)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_TypeDesc_TupleStruct(
  env,
  types,
) {
  let types = types.map(x => env.0.visit_Type(x))
  TupleStruct(types)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_TypeDesc_Alias(
  env,
  type_,
) {
  let type_ = env.0.visit_Type(type_)
  Alias(type_)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Hole(
  _,
  hole,
) {
  return hole
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_ArgumentKind(
  env,
  argument_kind,
) {
  match argument_kind {
    Positional => Positional
    Labelled(label) => env.0.visit_ArgumentKind_Labelled(label)
    LabelledPun(label) => env.0.visit_ArgumentKind_LabelledPun(label)
    LabelledOption(label~, question_loc~) =>
      env.0.visit_ArgumentKind_LabelledOption(label~, question_loc~)
    LabelledOptionPun(label~, question_loc~) =>
      env.0.visit_ArgumentKind_LabelledOptionPun(label~, question_loc~)
  }
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_ArgumentKind_Labelled(
  _,
  label,
) {
  Labelled(label)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_ArgumentKind_LabelledPun(
  _,
  label,
) {
  LabelledPun(label)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_ArgumentKind_LabelledOption(
  _,
  label~,
  question_loc~,
) {
  LabelledOption(label~, question_loc~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_ArgumentKind_LabelledOptionPun(
  _,
  label~,
  question_loc~,
) {
  LabelledOptionPun(label~, question_loc~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_FnKind(
  _,
  fn_kind,
) {
  fn_kind
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Group(
  _,
  group,
) {
  group
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_TrailingMark(
  _,
  trailing_mark,
) {
  trailing_mark
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_ApplyAttr(
  _,
  apply_attr,
) {
  apply_attr
}

// Continue with remaining MapVisitorBase implementations that are missing

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_TypeVarConstraint(
  env,
  type_var_constraint,
) {
  let trait_ = env.0.visit_LongIdent(type_var_constraint.trait_)
  let loc = type_var_constraint.loc
  TypeVarConstraint::{ trait_, loc }
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_TypeVarBinder(
  env,
  type_var_binder,
) {
  let constraints = type_var_binder.constraints.map(x => {
    env.0.visit_TypeVarConstraint(x)
  })
  let name = type_var_binder.name
  let loc = type_var_binder.name_loc
  TypeVarBinder::{ name, constraints, name_loc: loc }
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_TypeDeclBinder(
  _,
  type_decl_binder,
) {
  type_decl_binder
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Binder(
  _,
  binder,
) {
  binder
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Var(
  env,
  var_,
) {
  let name = env.0.visit_LongIdent(var_.name)
  let loc = var_.loc
  Var::{ name, loc }
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_ConstructorExtraInfo(
  env,
  constructor_extra_info,
) {
  match constructor_extra_info {
    TypeName(type_name) => env.0.visit_ConstructorExtraInfo_TypeName(type_name)
    Package(package_) => env.0.visit_ConstructorExtraInfo_Package(package_)
    NoExtraInfo => NoExtraInfo
  }
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_ConstructorExtraInfo_TypeName(
  env,
  type_name,
) {
  let type_name = env.0.visit_TypeName(type_name)
  TypeName(type_name)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_ConstructorExtraInfo_Package(
  _,
  package_,
) {
  Package(package_)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Constructor(
  env,
  constructor_,
) {
  let name = env.0.visit_ConstrName(constructor_.name)
  let extra_info = env.0.visit_ConstructorExtraInfo(constructor_.extra_info)
  let loc = constructor_.loc
  Constructor::{ name, extra_info, loc }
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Accessor(
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
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Accessor_Label(
  env,
  label,
) {
  let label = env.0.visit_Label(label)
  Label(label)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Accessor_Index(
  _,
  tuple_index~,
  loc~,
) {
  Index(tuple_index~, loc~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Accessor_Newtype(
  _,
  loc~,
) {
  Newtype(loc~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_AliasTarget(
  env,
  alias_target,
) {
  let binder = env.0.visit_Binder(alias_target.binder)
  let target = alias_target.target
  AliasTarget::{ binder, target }
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Argument(
  env,
  argument,
) {
  let value = env.0.visit_Expr(argument.value)
  let kind = env.0.visit_ArgumentKind(argument.kind)
  Argument::{ value, kind }
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Parameter(
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
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Parameter_DiscardPositional(
  env,
  ty~,
  loc~,
) {
  let ty = ty.map(x => env.0.visit_Type(x))
  DiscardPositional(ty~, loc~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Parameter_Positional(
  env,
  binder~,
  ty~,
) {
  let binder = env.0.visit_Binder(binder)
  let ty = ty.map(x => env.0.visit_Type(x))
  Positional(binder~, ty~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Parameter_Labelled(
  env,
  binder~,
  ty~,
) {
  let binder = env.0.visit_Binder(binder)
  Labelled(binder~, ty~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Parameter_Optional(
  env,
  binder~,
  default~,
  ty~,
) {
  let binder = env.0.visit_Binder(binder)
  let default = env.0.visit_Expr(default)
  let ty = ty.map(x => env.0.visit_Type(x))
  Optional(binder~, default~, ty~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Parameter_QuestionOptional(
  env,
  binder~,
  ty~,
) {
  let binder = env.0.visit_Binder(binder)
  let ty = ty.map(x => env.0.visit_Type(x))
  QuestionOptional(binder~, ty~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Case(
  env,
  case,
) {
  let pattern = env.0.visit_Pattern(case.pattern)
  let guard_ = case.guard_.map(x => env.0.visit_Expr(x))
  let body = env.0.visit_Expr(case.body)
  Case::{ pattern, guard_, body }
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_SpreadableElem(
  env,
  spreadable_elem,
) {
  match spreadable_elem {
    Regular(expr) => env.0.visit_SpreadableElem_Regular(expr)
    Spread(expr~, loc~) => env.0.visit_SpreadableElem_Spread(expr~, loc~)
  }
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_SpreadableElem_Regular(
  env,
  expr,
) {
  let expr = env.0.visit_Expr(expr)
  Regular(expr)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_SpreadableElem_Spread(
  env,
  expr~,
  loc~,
) {
  let expr = env.0.visit_Expr(expr)
  Spread(expr~, loc~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_MapExprElem(
  env,
  mapexpr_elem,
) {
  let key = env.0.visit_Constant(mapexpr_elem.key)
  let expr = env.0.visit_Expr(mapexpr_elem.expr)
  let key_loc = mapexpr_elem.key_loc
  let loc = mapexpr_elem.loc
  MapExprElem::{ key, expr, key_loc, loc }
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_StaticAssertion(
  env,
  static_assertion,
) {
  let ty = env.0.visit_Type(static_assertion.ty)
  let trait_ = env.0.visit_LongIdent(static_assertion.trait_)
  let loc = static_assertion.loc
  let msg = static_assertion.msg
  StaticAssertion::{ ty, trait_, loc, msg }
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Func(
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
  let parameters = parameters.map(p => env.0.visit_Parameter(p))
  let body = env.0.visit_Expr(body)
  let return_type = return_type.map(t => env.0.visit_Type(t))
  let error_type = env.0.visit_ErrorType(error_type)
  let kind = env.0.visit_FnKind(kind)
  {
    is_async,
    parameters,
    params_loc,
    body,
    return_type,
    error_type,
    kind,
    has_error,
    loc,
  }
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_FieldDef(
  env,
  field_def,
) {
  let label = env.0.visit_Label(field_def.label)
  let expr = env.0.visit_Expr(field_def.expr)
  let is_pun = field_def.is_pun
  let loc = field_def.loc
  FieldDef::{ label, expr, is_pun, loc }
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_InterpElem(
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
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_InterpElem_Literal(
  _,
  repr~,
  loc~,
) {
  Literal(repr~, loc~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_InterpElem_Expr(
  env,
  expr~,
  loc~,
) {
  let expr = env.0.visit_Expr(expr)
  Expr(expr~, loc~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_InterpElem_Source(
  _,
  interp_source,
) {
  Source(interp_source)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_MultilineStringElem(
  env,
  multiline_string_elem,
) {
  match multiline_string_elem {
    String(string_val) => env.0.visit_MultilineStringElem_String(string_val)
    Interp(interp_elems) => env.0.visit_MultilineStringElem_Interp(interp_elems)
  }
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_MultilineStringElem_String(
  _,
  string_val,
) {
  String(string_val)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_MultilineStringElem_Interp(
  _,
  interp_elems,
) {
  Interp(interp_elems)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_TryOperatorKind(
  _,
  try_operator_kind,
) {
  try_operator_kind
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_DotDotBinder(
  env,
  dot_dot_binder,
) {
  match dot_dot_binder {
    Underscore => Underscore
    NoBinder => NoBinder
    BinderAs(binder) => env.0.visit_DotDotBinder_BinderAs(binder)
    Binder(binder) => env.0.visit_DotDotBinder_Binder(binder)
  }
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_DotDotBinder_BinderAs(
  env,
  binder,
) {
  let binder = env.0.visit_Binder(binder)
  BinderAs(binder)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_DotDotBinder_Binder(
  env,
  binder,
) {
  let binder = env.0.visit_Binder(binder)
  Binder(binder)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_ArrayPattern(
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
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_ArrayPattern_Pattern(
  env,
  pattern,
) {
  let pattern = env.0.visit_Pattern(pattern)
  Pattern(pattern)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_ArrayPattern_StringSpread(
  _,
  str~,
  loc~,
) {
  StringSpread(str~, loc~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_ArrayPattern_BytesSpread(
  _,
  bytes~,
  loc~,
) {
  BytesSpread(bytes~, loc~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_ArrayPattern_ConstSpread(
  env,
  binder~,
  pkg~,
  loc~,
) {
  let binder = env.0.visit_Binder(binder)
  ConstSpread(binder~, pkg~, loc~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_ArrayPatterns(
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
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_ArrayPatterns_Closed(
  env,
  array_patterns,
) {
  let array_patterns = array_patterns.map(x => env.0.visit_ArrayPattern(x))
  Closed(array_patterns)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_ArrayPatterns_Open(
  env,
  prefix_patterns,
  suffix_patterns,
  dot_dot_binder,
) {
  let prefix_patterns = prefix_patterns.map(x => env.0.visit_ArrayPattern(x))
  let suffix_patterns = suffix_patterns.map(x => env.0.visit_ArrayPattern(x))
  let dot_dot_binder = env.0.visit_DotDotBinder(dot_dot_binder)
  Open(prefix_patterns, suffix_patterns, dot_dot_binder)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_WhereClause(
  env,
  where_clause,
) {
  let fields = where_clause.fields.map(x => env.0.visit_FieldDef(x))
  let loc = where_clause.loc
  WhereClause::{ fields, loc }
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_FieldPat(
  env,
  field_pat,
) {
  let label = env.0.visit_Label(field_pat.label)
  let pattern = env.0.visit_Pattern(field_pat.pattern)
  let is_pun = field_pat.is_pun
  let loc = field_pat.loc
  FieldPat::{ label, pattern, is_pun, loc }
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_ConstrPatArg(
  env,
  constr_pat_arg,
) {
  let pat = env.0.visit_Pattern(constr_pat_arg.pat)
  let kind = env.0.visit_ArgumentKind(constr_pat_arg.kind)
  ConstrPatArg::{ pat, kind }
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_MapPatElem(
  env,
  mappat_elem,
) {
  let key = env.0.visit_Constant(mappat_elem.key)
  let pat = env.0.visit_Pattern(mappat_elem.pat)
  let match_absent = mappat_elem.match_absent
  let key_loc = mappat_elem.key_loc
  let loc = mappat_elem.loc
  MapPatElem::{ key, pat, match_absent, key_loc, loc }
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_RangeKind(
  env,
  kind,
) {
  ignore(env)
  kind
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Pattern(
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
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Pattern_Alias(
  env,
  pat~,
  alias_~,
  loc~,
) {
  let pat = env.0.visit_Pattern(pat)
  let alias_ = env.0.visit_Binder(alias_)
  Alias(pat~, alias_~, loc~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Pattern_Any(
  _,
  loc~,
) {
  Any(loc~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Pattern_Array(
  env,
  pats~,
  loc~,
) {
  let pats = env.0.visit_ArrayPatterns(pats)
  Array(pats~, loc~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Pattern_Constant(
  env,
  c~,
  loc~,
) {
  let c = env.0.visit_Constant(c)
  Constant(c~, loc~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Pattern_Constraint(
  env,
  pat~,
  ty~,
  loc~,
) {
  let pat = env.0.visit_Pattern(pat)
  let ty = env.0.visit_Type(ty)
  Constraint(pat~, ty~, loc~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Pattern_Constr(
  env,
  constr~,
  args~,
  is_open~,
  loc~,
) {
  let constr = env.0.visit_Constructor(constr)
  let args = args.map(args => args.map(x => env.0.visit_ConstrPatArg(x)))
  Constr(constr~, args~, is_open~, loc~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Pattern_Or(
  env,
  pat1~,
  pat2~,
  loc~,
) {
  let pat1 = env.0.visit_Pattern(pat1)
  let pat2 = env.0.visit_Pattern(pat2)
  Or(pat1~, pat2~, loc~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Pattern_Tuple(
  env,
  pats~,
  loc~,
) {
  let pats = pats.map(x => env.0.visit_Pattern(x))
  Tuple(pats~, loc~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Pattern_Var(
  env,
  binder,
) {
  let binder = env.0.visit_Binder(binder)
  Var(binder)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Pattern_Record(
  env,
  fields~,
  is_closed~,
  loc~,
) {
  let fields = fields.map(x => env.0.visit_FieldPat(x))
  Record(fields~, is_closed~, loc~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Pattern_Map(
  env,
  elems~,
  is_closed~,
  loc~,
) {
  let elems = elems.map(x => env.0.visit_MapPatElem(x))
  Map(elems~, is_closed~, loc~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Pattern_Range(
  env,
  lhs~,
  rhs~,
  kind~,
  loc~,
) {
  let lhs = env.0.visit_Pattern(lhs)
  let rhs = env.0.visit_Pattern(rhs)
  let kind = env.0.visit_RangeKind(kind)
  Range(lhs~, rhs~, kind~, loc~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Pattern_SpecialConstr(
  env,
  binder~,
  args~,
  loc~,
) {
  let binder = env.0.visit_Binder(binder)
  let args = args.map(x => env.0.visit_ConstrPatArg(x))
  SpecialConstr(binder~, args~, loc~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_LocalTypeDecl(
  env,
  localtype_decl,
) {
  let components = env.0.visit_TypeDesc(localtype_decl.components)
  let deriving = localtype_decl.deriving.map(x => {
    env.0.visit_DerivingDirective(x)
  })
  let tycon = localtype_decl.tycon
  let tycon_loc = localtype_decl.tycon_loc
  LocalTypeDecl::{ tycon, tycon_loc, components, deriving }
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_DerivingDirective(
  env,
  deriving_directive,
) {
  let type_name = env.0.visit_TypeName(deriving_directive.type_name)
  let args = deriving_directive.args.map(x => env.0.visit_Argument(x))
  let loc = deriving_directive.loc
  DerivingDirective::{ type_name, args, loc }
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_TypeDecl(
  env,
  type_decl,
) {
  let params = type_decl.params.map(x => env.0.visit_TypeDeclBinder(x))
  let components = env.0.visit_TypeDesc(type_decl.components)
  let deriving = type_decl.deriving.map(x => env.0.visit_DerivingDirective(x))
  let type_vis = env.0.visit_Visibility(type_decl.type_vis)
  let tycon = type_decl.tycon
  let tycon_loc = type_decl.tycon_loc
  let attrs = type_decl.attrs
  let doc = type_decl.doc
  let loc = type_decl.loc
  let is_declare = type_decl.is_declare
  TypeDecl::{
    tycon,
    tycon_loc,
    params,
    components,
    attrs,
    doc,
    type_vis,
    deriving,
    loc,
    is_declare,
  }
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_FuncStubs(
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
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_FuncStubs_Import(
  _,
  module_name~,
  func_name~,
  language~,
) {
  Import(module_name~, func_name~, language~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_FuncStubs_Embedded(
  env,
  language~,
  code~,
) {
  let code = env.0.visit_EmbeddedCode(code)
  Embedded(language~, code~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_EmbeddedCode(
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
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_EmbeddedCode_CodeString(
  _,
  string_literal,
) {
  CodeString(string_literal)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_EmbeddedCode_CodeMultilineString(
  _,
  strings,
) {
  CodeMultilineString(strings)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_DeclBody(
  env,
  decl_body,
) {
  match decl_body {
    DeclBody(local_types~, expr~) =>
      env.0.visit_DeclBody_DeclBody(local_types~, expr~)
    DeclStubs(func_stubs) => env.0.visit_DeclBody_DeclStubs(func_stubs)
    DeclNone => DeclNone
  }
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_DeclBody_DeclBody(
  env,
  local_types~,
  expr~,
) {
  let local_types = local_types.map(x => env.0.visit_LocalTypeDecl(x))
  let expr = env.0.visit_Expr(expr)
  DeclBody(local_types~, expr~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_DeclBody_DeclStubs(
  env,
  func_stubs,
) {
  let func_stubs = env.0.visit_FuncStubs(func_stubs)
  DeclStubs(func_stubs)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_FunDecl(
  env,
  fun_decl,
) {
  let type_name = fun_decl.type_name.map(x => env.0.visit_TypeName(x))
  let name = env.0.visit_Binder(fun_decl.name)
  let decl_params = fun_decl.decl_params.map(decl_params => {
    decl_params.map(param => env.0.visit_Parameter(param))
  })
  let quantifiers = fun_decl.quantifiers.map(x => env.0.visit_TypeVarBinder(x))
  let return_type = fun_decl.return_type.map(x => env.0.visit_Type(x))
  let error_type = env.0.visit_ErrorType(fun_decl.error_type)
  let vis = env.0.visit_Visibility(fun_decl.vis)
  let has_error = fun_decl.has_error
  let is_async = fun_decl.is_async
  let params_loc = fun_decl.params_loc
  let attrs = fun_decl.attrs
  let doc = fun_decl.doc
  FunDecl::{
    type_name,
    name,
    has_error,
    is_async,
    decl_params,
    params_loc,
    quantifiers,
    return_type,
    error_type,
    vis,
    attrs,
    doc,
  }
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_TraitMethodDecl(
  env,
  method_decl,
) {
  let name = env.0.visit_Binder(method_decl.name)
  let quantifiers = method_decl.quantifiers.map(x => {
    env.0.visit_TypeVarBinder(x)
  })
  let params = method_decl.params.map(param => env.0.visit_Parameter(param))
  let return_type = method_decl.return_type.map(x => env.0.visit_Type(x))
  let error_type = env.0.visit_ErrorType(method_decl.error_type)
  let has_error = method_decl.has_error
  let is_async = method_decl.is_async
  let has_default = method_decl.has_default
  let attrs = method_decl.attrs
  let loc = method_decl.loc
  TraitMethodDecl::{
    name,
    has_error,
    is_async,
    quantifiers,
    params,
    return_type,
    error_type,
    has_default,
    attrs,
    loc,
  }
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_TraitDecl(
  env,
  trait_decl,
) {
  let name = env.0.visit_Binder(trait_decl.name)
  let supers = trait_decl.supers.map(x => env.0.visit_TypeVarConstraint(x))
  let methods = trait_decl.methods.map(x => env.0.visit_TraitMethodDecl(x))
  let vis = env.0.visit_Visibility(trait_decl.vis)
  let loc = trait_decl.loc
  let attrs = trait_decl.attrs
  let doc = trait_decl.doc
  let is_declare = trait_decl.is_declare
  TraitDecl::{ name, supers, methods, vis, loc, attrs, doc, is_declare }
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Impl_TopExpr(
  env,
  expr~,
  is_main~,
  local_types~,
  is_async~,
  loc~,
) {
  let expr = env.0.visit_Expr(expr)
  let local_types = local_types.map(x => env.0.visit_LocalTypeDecl(x))
  TopExpr(expr~, is_main~, local_types~, is_async~, loc~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Impl_TopTest(
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
  let expr = env.0.visit_Expr(expr)
  let params = params.map(params => {
    params.map(param => env.0.visit_Parameter(param))
  })
  let local_types = local_types.map(x => env.0.visit_LocalTypeDecl(x))
  TopTest(expr~, name~, params~, local_types~, is_async~, loc~, attrs~, doc~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Impl_TopTypeDef(
  env,
  type_decl,
) {
  let type_decl = env.0.visit_TypeDecl(type_decl)
  TopTypeDef(type_decl)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Impl_TopFuncDef(
  env,
  fun_decl~,
  decl_body~,
  loc~,
) {
  let fun_decl = env.0.visit_FunDecl(fun_decl)
  let decl_body = env.0.visit_DeclBody(decl_body)
  TopFuncDef(fun_decl~, decl_body~, loc~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Impl_TopFuncAlias(
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
  let pkg = pkg.map(x => env.0.visit_Label(x))
  let type_name = type_name.map(x => env.0.visit_Label(x))
  let targets = targets.map(x => env.0.visit_AliasTarget(x))
  let vis = env.0.visit_Visibility(vis)
  TopFuncAlias(pkg~, type_name~, targets~, vis~, attrs~, is_list~, doc~, loc~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Impl_TopLetDef(
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
  let binder = env.0.visit_Binder(binder)
  let ty = ty.map(x => env.0.visit_Type(x))
  let expr = env.0.visit_Expr(expr)
  let vis = env.0.visit_Visibility(vis)
  TopLetDef(
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
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Impl_TopTrait(
  env,
  trait_decl,
) {
  let trait_decl = env.0.visit_TraitDecl(trait_decl)
  TopTrait(trait_decl)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Impl_TopBatchTypeAlias(
  env,
  pkg~,
  targets~,
  vis~,
  loc~,
  attrs~,
  is_list~,
  doc~,
) {
  let pkg = pkg.map(x => env.0.visit_Label(x))
  let targets = targets.map(x => env.0.visit_AliasTarget(x))
  let vis = env.0.visit_Visibility(vis)
  TopBatchTypeAlias(pkg~, targets~, vis~, loc~, attrs~, is_list~, doc~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Impl_TopBatchTraitAlias(
  env,
  pkg~,
  targets~,
  vis~,
  loc~,
  attrs~,
  is_list~,
  doc~,
) {
  let pkg = pkg.map(x => env.0.visit_Label(x))
  let targets = targets.map(x => env.0.visit_AliasTarget(x))
  let vis = env.0.visit_Visibility(vis)
  TopBatchTraitAlias(pkg~, targets~, vis~, loc~, attrs~, is_list~, doc~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Impl_TopImpl(
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
  let self_ty = self_ty.map(x => env.0.visit_Type(x))
  let trait_ = env.0.visit_TypeName(trait_)
  let method_name = env.0.visit_Binder(method_name)
  let quantifiers = quantifiers.map(x => env.0.visit_TypeVarBinder(x))
  let params = params.map(param => env.0.visit_Parameter(param))
  let ret_ty = ret_ty.map(x => env.0.visit_Type(x))
  let err_ty = env.0.visit_ErrorType(err_ty)
  let body = env.0.visit_DeclBody(body)
  let vis = env.0.visit_Visibility(vis)
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
    doc~,
  )
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Impl_TopView(
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
  let quantifiers = quantifiers.map(x => env.0.visit_TypeVarBinder(x))
  let source_ty = env.0.visit_Type(source_ty)
  let view_constrs = view_constrs.map(x => env.0.visit_ConstrDecl(x))
  let view_func_name = env.0.visit_Binder(view_func_name)
  let parameters = parameters.map(param => env.0.visit_Parameter(param))
  let body = env.0.visit_Expr(body)
  let vis = env.0.visit_Visibility(vis)
  TopView(
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
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Impl_TopImplRelation(
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
  let self_ty = env.0.visit_Type(self_ty)
  let trait_ = env.0.visit_TypeName(trait_)
  let quantifiers = quantifiers.map(x => env.0.visit_TypeVarBinder(x))
  let vis = env.0.visit_Visibility(vis)
  let is_declare = is_declare
  TopImplRelation(
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
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Impl_TopUsing(
  env,
  pkg~,
  names~,
  vis~,
  attrs~,
  loc~,
  doc~,
) {
  let pkg = env.0.visit_Label(pkg)
  let names = names.map(name => {
    (env.0.visit_AliasTarget(name.0), env.0.visit_UsingKind(name.1))
  })
  let vis = env.0.visit_Visibility(vis)
  TopUsing(pkg~, names~, vis~, attrs~, loc~, doc~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Expr_Apply(
  env,
  func~,
  args~,
  attr~,
  loc~,
) {
  let func = env.0.visit_Expr(func)
  let args = args.map(x => env.0.visit_Argument(x))
  Expr::Apply(func~, args~, attr~, loc~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Expr_Infix(
  env,
  op~,
  lhs~,
  rhs~,
  loc~,
) {
  let op = env.0.visit_Var(op)
  let lhs = env.0.visit_Expr(lhs)
  let rhs = env.0.visit_Expr(rhs)
  Expr::Infix(op~, lhs~, rhs~, loc~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Expr_Unary(
  env,
  op~,
  expr~,
  loc~,
) {
  let op = env.0.visit_Var(op)
  let expr = env.0.visit_Expr(expr)
  Expr::Unary(op~, expr~, loc~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Expr_Array(
  env,
  exprs~,
  loc~,
) {
  let exprs = exprs.map(expr => env.0.visit_Expr(expr))
  Expr::Array(exprs~, loc~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Expr_ArraySpread(
  env,
  elems~,
  loc~,
) {
  let elems = elems.map(fn(elem) {
    match elem {
      Regular(expr) => Regular(env.0.visit_Expr(expr))
      Spread(expr~, loc~) => Spread(expr=env.0.visit_Expr(expr), loc~)
    }
  })
  Expr::ArraySpread(elems~, loc~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Expr_ArrayGet(
  env,
  array~,
  index~,
  loc~,
) {
  let array = env.0.visit_Expr(array)
  let index = env.0.visit_Expr(index)
  Expr::ArrayGet(array~, index~, loc~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Expr_ArrayGetSlice(
  env,
  array~,
  start_index~,
  end_index~,
  index_loc~,
  loc~,
) {
  let array = env.0.visit_Expr(array)
  let start_index = start_index.map(e => env.0.visit_Expr(e))
  let end_index = end_index.map(e => env.0.visit_Expr(e))
  Expr::ArrayGetSlice(array~, start_index~, end_index~, index_loc~, loc~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Expr_ArraySet(
  env,
  array~,
  index~,
  value~,
  loc~,
) {
  let index = env.0.visit_Expr(index)
  let value = env.0.visit_Expr(value)
  let array = env.0.visit_Expr(array)
  Expr::ArraySet(array~, index~, value~, loc~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Expr_ArrayAugmentedSet(
  env,
  op~,
  array~,
  index~,
  value~,
  loc~,
) {
  let op = env.0.visit_Var(op)
  let array = env.0.visit_Expr(array)
  let index = env.0.visit_Expr(index)
  let value = env.0.visit_Expr(value)
  Expr::ArrayAugmentedSet(op~, array~, index~, value~, loc~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Expr_Constant(
  env,
  c~,
  loc~,
) {
  let c = env.0.visit_Constant(c)
  Expr::Constant(c~, loc~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Expr_MultilineString(
  env,
  elems~,
  loc~,
) {
  let elems = elems.map(elem => env.0.visit_MultilineStringElem(elem))
  Expr::MultilineString(elems~, loc~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Expr_Interp(
  env,
  elems~,
  loc~,
) {
  let elems = elems.map(elem => env.0.visit_InterpElem(elem))
  Expr::Interp(elems~, loc~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Expr_Constraint(
  env,
  expr~,
  ty~,
  loc~,
) {
  let ty = env.0.visit_Type(ty)
  let expr = env.0.visit_Expr(expr)
  Expr::Constraint(expr~, ty~, loc~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Expr_Constr(
  env,
  constr~,
  loc~,
) {
  let constr = env.0.visit_Constructor(constr)
  Expr::Constr(constr~, loc~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Expr_While(
  env,
  loop_cond~,
  loop_body~,
  while_else~,
  label~,
  loc~,
) {
  let loop_cond = env.0.visit_Expr(loop_cond)
  let loop_body = env.0.visit_Expr(loop_body)
  let while_else = while_else.map(else_block => env.0.visit_Expr(else_block))
  let label = label.map(x => env.0.visit_Label(x))
  Expr::While(loop_cond~, loop_body~, while_else~, label~, loc~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Expr_Function(
  env,
  func~,
  loc~,
) {
  let func = env.0.visit_Func(func)
  Expr::Function(func~, loc~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Expr_Ident(
  env,
  id~,
  loc~,
) {
  let id = env.0.visit_Var(id)
  Expr::Ident(id~, loc~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Expr_If(
  env,
  cond~,
  ifso~,
  ifnot~,
  loc~,
) {
  let cond = env.0.visit_Expr(cond)
  let ifso = env.0.visit_Expr(ifso)
  let ifnot = ifnot.map(expr => env.0.visit_Expr(expr))
  Expr::If(cond~, ifso~, ifnot~, loc~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Expr_Guard(
  env,
  cond~,
  otherwise~,
  body~,
  loc~,
) {
  let cond = env.0.visit_Expr(cond)
  let body = env.0.visit_Expr(body)
  let otherwise = otherwise.map(expr => env.0.visit_Expr(expr))
  Expr::Guard(cond~, otherwise~, body~, loc~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Expr_Is(
  env,
  expr~,
  pat~,
  loc~,
) {
  let expr = env.0.visit_Expr(expr)
  let pat = env.0.visit_Pattern(pat)
  Expr::Is(expr~, pat~, loc~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Expr_Defer(
  env,
  expr~,
  body~,
  loc~,
) {
  let expr = env.0.visit_Expr(expr)
  let body = env.0.visit_Expr(body)
  Expr::Defer(expr~, body~, loc~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Expr_LetFn(
  env,
  name~,
  func~,
  body~,
  loc~,
) {
  let name = env.0.visit_Binder(name)
  let func = env.0.visit_Func(func)
  let body = env.0.visit_Expr(body)
  Expr::LetFn(name~, func~, body~, loc~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Expr_LetRec(
  env,
  bindings~,
  body~,
  loc~,
) {
  let bindings = bindings.map(binding => {
    match binding {
      (name, func) => (env.0.visit_Binder(name), env.0.visit_Func(func))
    }
  })
  let body = env.0.visit_Expr(body)
  Expr::LetRec(bindings~, body~, loc~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Expr_LetAnd(
  env,
  bindings~,
  body~,
  loc~,
) {
  let bindings = bindings.map(binding => {
    match binding {
      (binder, ty, func) =>
        (
          env.0.visit_Binder(binder),
          ty.map(x => env.0.visit_Type(x)),
          env.0.visit_Func(func),
        )
    }
  })
  let body = env.0.visit_Expr(body)
  Expr::LetAnd(bindings~, body~, loc~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Expr_Let(
  env,
  pattern~,
  expr~,
  body~,
  loc~,
) {
  let pattern = env.0.visit_Pattern(pattern)
  let expr = env.0.visit_Expr(expr)
  let body = env.0.visit_Expr(body)
  Expr::Let(pattern~, expr~, body~, loc~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Expr_Sequence(
  env,
  exprs~,
  last_expr~,
  loc~,
) {
  let exprs = exprs.map(expr => env.0.visit_Expr(expr))
  let last_expr = env.0.visit_Expr(last_expr)
  Expr::Sequence(exprs~, last_expr~, loc~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Expr_Tuple(
  env,
  exprs~,
  loc~,
) {
  let exprs = exprs.map(expr => env.0.visit_Expr(expr))
  Expr::Tuple(exprs~, loc~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Expr_Record(
  env,
  type_name~,
  fields~,
  trailing~,
  loc~,
) {
  let type_name = type_name.map(x => env.0.visit_TypeName(x))
  let fields = fields.map(x => env.0.visit_FieldDef(x))
  let trailing = env.0.visit_TrailingMark(trailing)
  Expr::Record(type_name~, fields~, trailing~, loc~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Expr_RecordUpdate(
  env,
  type_name~,
  record~,
  fields~,
  loc~,
) {
  let type_name = type_name.map(x => env.0.visit_TypeName(x))
  let fields = fields.map(x => env.0.visit_FieldDef(x))
  let record = env.0.visit_Expr(record)
  Expr::RecordUpdate(type_name~, record~, fields~, loc~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Expr_Field(
  env,
  record~,
  accessor~,
  loc~,
) {
  let record = env.0.visit_Expr(record)
  let accessor = env.0.visit_Accessor(accessor)
  Expr::Field(record~, accessor~, loc~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Expr_Method(
  env,
  type_name~,
  method_name~,
  loc~,
) {
  let type_name = env.0.visit_TypeName(type_name)
  let method_name = env.0.visit_Label(method_name)
  Expr::Method(type_name~, method_name~, loc~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Expr_DotApply(
  env,
  self~,
  method_name~,
  args~,
  return_self~,
  attr~,
  loc~,
) {
  let self = env.0.visit_Expr(self)
  let method_name = env.0.visit_Label(method_name)
  let args = args.map(x => env.0.visit_Argument(x))
  Expr::DotApply(self~, method_name~, args~, return_self~, attr~, loc~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Expr_As(
  env,
  expr~,
  trait_~,
  loc~,
) {
  let trait_ = env.0.visit_TypeName(trait_)
  let expr = env.0.visit_Expr(expr)
  Expr::As(expr~, trait_~, loc~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Expr_Mutate(
  env,
  record~,
  accessor~,
  field~,
  augmented_by~,
  loc~,
) {
  let record = env.0.visit_Expr(record)
  let accessor = env.0.visit_Accessor(accessor)
  let field = env.0.visit_Expr(field)
  let augmented_by = augmented_by.map(x => env.0.visit_Var(x))
  Expr::Mutate(record~, accessor~, field~, augmented_by~, loc~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Expr_Match(
  env,
  expr~,
  cases~,
  match_loc~,
  loc~,
) {
  let expr = env.0.visit_Expr(expr)
  let cases = cases.map(x => env.0.visit_Case(x))
  Expr::Match(expr~, cases~, match_loc~, loc~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Expr_LetMut(
  env,
  binder~,
  ty~,
  expr~,
  body~,
  loc~,
) {
  let binder = env.0.visit_Binder(binder)
  let ty = ty.map(x => env.0.visit_Type(x))
  let expr = env.0.visit_Expr(expr)
  let body = env.0.visit_Expr(body)
  Expr::LetMut(binder~, ty~, expr~, body~, loc~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Expr_Pipe(
  env,
  lhs~,
  rhs~,
  loc~,
) {
  let lhs = env.0.visit_Expr(lhs)
  let rhs = env.0.visit_Expr(rhs)
  Expr::Pipe(lhs~, rhs~, loc~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Expr_Assign(
  env,
  var_~,
  expr~,
  augmented_by~,
  loc~,
) {
  let var_ = env.0.visit_Var(var_)
  let augmented_by = augmented_by.map(x => env.0.visit_Var(x))
  let expr = env.0.visit_Expr(expr)
  Expr::Assign(var_~, expr~, augmented_by~, loc~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Expr_Hole(
  env,
  loc~,
  kind~,
) {
  let kind = env.0.visit_Hole(kind)
  Expr::Hole(loc~, kind~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Expr_Return(
  env,
  return_value~,
  loc~,
) {
  let return_value = return_value.map(value => env.0.visit_Expr(value))
  Expr::Return(return_value~, loc~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Expr_Raise(
  env,
  err_value~,
  loc~,
) {
  let err_value = env.0.visit_Expr(err_value)
  Expr::Raise(err_value~, loc~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Expr_Unit(
  _,
  loc~,
  faked~,
) {
  Expr::Unit(loc~, faked~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Expr_Break(
  env,
  arg~,
  label~,
  loc~,
) {
  let label = label.map(x => env.0.visit_Label(x))
  let arg = arg.map(x => env.0.visit_Expr(x))
  Expr::Break(arg~, label~, loc~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Expr_Continue(
  env,
  args~,
  label~,
  loc~,
) {
  let args = args.map(arg => env.0.visit_Expr(arg))
  let label = label.map(x => env.0.visit_Label(x))
  Expr::Continue(args~, label~, loc~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Expr_Loop(
  env,
  arg~,
  body~,
  label~,
  loop_loc~,
  loc~,
) {
  let arg = env.0.visit_Expr(arg)
  let body = body.map(x => env.0.visit_Case(x))
  let label = label.map(x => env.0.visit_Label(x))
  Expr::Loop(arg~, body~, label~, loop_loc~, loc~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Expr_For(
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
  let binders = binders.map(p => {
    (env.0.visit_Binder(p.0), env.0.visit_Expr(p.1))
  })
  let condition = condition.map(cond => env.0.visit_Expr(cond))
  let continue_block = continue_block.map(p => {
    (env.0.visit_Binder(p.0), env.0.visit_Expr(p.1))
  })
  let body = env.0.visit_Expr(body)
  let for_else = for_else.map(else_block => env.0.visit_Expr(else_block))
  let label = label.map(x => env.0.visit_Label(x))
  let where_clause = where_clause.map(x => env.0.visit_WhereClause(x))
  Expr::For(
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
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Expr_ForEach(
  env,
  binders~,
  expr~,
  body~,
  else_block~,
  label~,
  loc~,
) {
  let binders = binders.map(binder => binder.map(x => env.0.visit_Binder(x)))
  let expr = env.0.visit_Expr(expr)
  let body = env.0.visit_Expr(body)
  let else_block = else_block.map(else_block => env.0.visit_Expr(else_block))
  let label = label.map(x => env.0.visit_Label(x))
  Expr::ForEach(binders~, expr~, body~, else_block~, label~, loc~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Expr_Try(
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
  let body = env.0.visit_Expr(body)
  let catch_ = catch_.map(x => env.0.visit_Case(x))
  let try_else = try_else.map(try_else => try_else.map(x => env.0.visit_Case(x)))
  Expr::Try(
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
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Expr_TryOperator(
  env,
  body~,
  kind~,
  try_loc~,
  loc~,
) {
  let body = env.0.visit_Expr(body)
  let kind = env.0.visit_TryOperatorKind(kind)
  Expr::TryOperator(body~, kind~, try_loc~, loc~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Expr_Map(
  env,
  elems~,
  loc~,
) {
  let elems = elems.map(x => env.0.visit_MapExprElem(x))
  Expr::Map(elems~, loc~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Expr_Group(
  env,
  expr~,
  group~,
  loc~,
) {
  let expr = env.0.visit_Expr(expr)
  let group = env.0.visit_Group(group)
  Expr::Group(expr~, group~, loc~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Expr_StaticAssert(
  env,
  asserts~,
  body~,
) {
  let asserts = asserts.map(x => env.0.visit_StaticAssertion(x))
  let body = env.0.visit_Expr(body)
  Expr::StaticAssert(asserts~, body~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Expr(
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
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_UsingKind(
  env,
  using_kind,
) {
  ignore(env)
  return using_kind
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Expr_IsLexMatch(
  env,
  expr~,
  strategy~,
  pat~,
  pat_loc~,
  loc~,
) {
  let strategy = strategy.map(strategy => env.0.visit_Label(strategy))
  let pat = pat.map(pat => env.0.visit_LexTopPattern(pat))
  let expr = env.0.visit_Expr(expr)
  IsLexMatch(strategy~, expr~, pat~, pat_loc~, loc~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_Expr_LexMatch(
  env,
  strategy~,
  expr~,
  match_loc~,
  cases~,
  loc~,
) {
  let strategy = strategy.map(strategy => env.0.visit_Label(strategy))
  let expr = env.0.visit_Expr(expr)
  let cases = cases.map(case => env.0.visit_LexCase(case))
  Expr::LexMatch(strategy~, expr~, match_loc~, cases~, loc~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_LexCase(
  env,
  lex_case,
) {
  let pat = lex_case.pat.map(pat => env.0.visit_LexTopPattern(pat))
  let body = env.0.visit_Expr(lex_case.body)
  let pat_loc = lex_case.pat_loc
  { pat, body, pat_loc }
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_LexTopPattern(
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
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_LexTopPattern_Pattern(
  env,
  lex_pattern,
) {
  let lex_pattern = env.0.visit_LexPattern(lex_pattern)
  LexTopPattern::Pattern(lex_pattern)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_LexTopPattern_Binder(
  env,
  binder,
) {
  let binder = env.0.visit_Binder(binder)
  LexTopPattern::Binder(binder)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_LexTopPattern_Wildcard(
  env,
  loc~,
) {
  ignore(env)
  LexTopPattern::Wildcard(loc~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_LexPattern(
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
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_LexPattern_Regex(
  env,
  lit~,
  loc~,
) {
  ignore(env)
  LexPattern::Regex(lit~, loc~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_LexPattern_RegexInterp(
  env,
  elems~,
  loc~,
) {
  let elems = elems.map(elem => env.0.visit_InterpElem(elem))
  LexPattern::RegexInterp(elems~, loc~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_LexPattern_Alias(
  env,
  pat~,
  binder~,
  loc~,
) {
  let pat = env.0.visit_LexPattern(pat)
  let binder = env.0.visit_Binder(binder)
  LexPattern::Alias(pat~, binder~, loc~)
}

///|
pub impl[T : MapVisitor] MapVisitor for MapVisitorBase[T] with visit_LexPattern_Sequence(
  env,
  pats~,
  loc~,
) {
  let pats = pats.map(pat => env.0.visit_LexPattern(pat))
  LexPattern::Sequence(pats~, loc~)
}
