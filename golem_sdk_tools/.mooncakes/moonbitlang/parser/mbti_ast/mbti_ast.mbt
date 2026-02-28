///|
pub(all) struct Name {
  name : String
  loc : Location
} derive(ToJson)

///|
pub(all) struct QualifiedName {
  name : QualifiedIdent
  loc : Location
}

///|
pub(all) struct TypeParamWithConstraints {
  name : Name
  constraints : @list.List[QualifiedName]
}

///|
pub(all) enum TypeParamNoConstraints {
  Name(Name)
  Underscore(Location)
}

///|
pub(all) enum Parameter {
  Positional(Type)
  Labelled(Label, Type)
  Autofill(Label, Type)
  OptionalDefault(Label, Type)
  OptionalOption(Label, Type)
}

///|
pub(all) enum TraitMethodParameter {
  Positional(Type)
  Labelled(Label, Type)
}

///|
pub(all) struct FuncSig {
  attr : @list.List[(String, String?, String)]
  type_name : Name?
  name : Name
  params : @list.List[Parameter]
  return_ : (Type, ErrorType)
  type_params : @list.List[TypeParamWithConstraints]
}

///|
pub(all) struct TypeSig {
  name : Name
  type_params : @list.List[TypeParamNoConstraints]
  components : TypeDesc
  vis : Visibility
}

///|
pub(all) enum AliasSig {
  TypeAlias(
    name~ : Name,
    type_params~ : @list.List[TypeParamNoConstraints],
    type_~ : Type,
    vis~ : Visibility
  )
  TraitAlias(name~ : Name, trait_name~ : QualifiedName, vis~ : Visibility)
  FnAlias(name~ : Name, type_name~ : QualifiedName, loc~ : Location)
}

///|
pub(all) struct TraitMethodSig {
  name : Name
  params : @list.List[TraitMethodParameter]
  has_default : Bool
  return_ : (Type, ErrorType)
}

///|
pub(all) struct TraitSig {
  name : Name
  super_traits : @list.List[QualifiedName]
  methods : @list.List[TraitMethodSig]
  vis : Visibility
}

///|
pub(all) enum ImplSig {
  Trait(
    type_params~ : @list.List[TypeParamWithConstraints],
    type_~ : Type,
    trait_name~ : QualifiedName
  )
  DefaultImpl(trait_name~ : Name, method_name~ : Name)
}

///|
pub(all) struct ConstSig {
  name : Name
  type_ : Type
  value : Constant
}

///|
pub(all) struct ValueSig {
  name : Name
  type_ : Type
}

///|
pub(all) struct PackageImport {
  name : String
  alias_ : String?
}

///|
pub(all) enum Sig {
  Func(FuncSig)
  Type(TypeSig)
  Alias(AliasSig)
  Trait(TraitSig)
  Impl(ImplSig)
  Const(ConstSig)
  Value(ValueSig)
}

///|
pub(all) struct Mbti {
  package_name : String
  imports : @list.List[PackageImport]
  sigs : @list.List[(Sig, Location)]
}
