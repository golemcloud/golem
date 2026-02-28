///|
using @tokens {type Token, type TokenKind}

///|
using @basic {type Location, type Position}

///|
using @tokens {
  type Comment,
  type CharLiteral,
  type StringLiteral,
  type InterpLiteral,
}

///|
using @syntax {type DocString}

///|
using @attribute {type Attribute}

///|
using @mbti_ast {
  type QualifiedIdent,
  type Name,
  type QualifiedName,
  type Label,
  type Visibility,
  type Constant,
  type Type,
  type ErrorType,
  type Parameter,
  type TypeParamNoConstraints,
  type TypeParamWithConstraints,
  type TraitMethodParameter,
  type FieldDecl,
  type FieldName,
  type ConstrDecl,
  type ConstrId,
  type ConstrName,
  type ConstrParam,
  type AliasSig,
  type TraitSig,
  type TraitMethodSig,
  type ImplSig,
  type FuncSig,
  type ValueSig,
  type ConstSig,
  type TypeSig,
  type Sig,
  type PackageImport,
  type Mbti,
}
