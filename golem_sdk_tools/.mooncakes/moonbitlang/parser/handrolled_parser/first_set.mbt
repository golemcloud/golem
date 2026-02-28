///|
let first_qual_lident : Array[TokenKind] = [TK_LIDENT, TK_PACKAGE_NAME]

///|
let first_qual_luident : Array[TokenKind] = [..first_qual_lident, TK_UIDENT]

///|
let first_luident : Array[TokenKind] = [TK_LIDENT, TK_UIDENT]

///|
let first_constr : Array[TokenKind] = first_luident

///|
let first_impl_ : Array[TokenKind] = [
  TK_PUB,
  TK_PRIV,
  TK_TYPE,
  TK_SUBERROR,
  TK_TYPEALIAS,
  TK_ASYNC,
  TK_FN,
  TK_FNALIAS,
  TK_STRUCT,
  TK_ENUM,
  TK_LET,
  TK_CONST,
  TK_EXTERN,
  TK_TEST,
  TK_IMPL,
  TK_TRAIT,
  TK_TRAITALIAS,
  TK_ENUMVIEW,
  TK_ATTRIBUTE,
  TK_USING,
]

///|
let first_simple_expr : Array[TokenKind] = [
  TK_TRUE,
  TK_FALSE,
  TK_INT,
  TK_FLOAT,
  TK_DOUBLE,
  TK_CHAR,
  TK_STRING,
  TK_MULTILINE_STRING,
  TK_MULTILINE_INTERP,
  TK_BYTE,
  TK_INTERP,
  TK_PACKAGE_NAME,
  TK_ASYNC,
  TK_FN,
  TK_LIDENT,
  TK_UIDENT,
  TK_LPAREN,
  TK_LBRACKET,
  TK_LBRACE,
  TK_UNDERSCORE,
  TK_EXCLAMATION,
]

///|
let first_expr : Array[TokenKind] = [
  ..first_simple_expr,
  TK_IF,
  TK_MATCH,
  TK_PLUS,
  TK_MINUS,
  TK_LOOP,
  TK_WHILE,
  TK_FOR,
  TK_POST_LABEL,
  TK_GUARD,
  TK_TRY,
  TK_DEFER,
  TK_TRY_QUESTION,
  TK_TRY_EXCLAMATION,
  TK_UNDERSCORE,
  TK_LPAREN,
  TK_LIDENT,
  TK_EXCLAMATION,
]

///|
let first_map_pattern_key : Array[TokenKind] = [
  TK_TRUE,
  TK_FALSE,
  TK_CHAR,
  TK_INT,
  TK_FLOAT,
  TK_DOUBLE,
  TK_MINUS,
  TK_STRING,
]

///|
let first_type_ : Array[TokenKind] = [
  TK_PACKAGE_NAME,
  TK_LIDENT,
  TK_UIDENT,
  TK_LPAREN,
  TK_UNDERSCORE,
  TK_AMPER,
  TK_ASYNC,
]

///|
let first_simple_pattern : Array[TokenKind] = [
  TK_TRUE,
  TK_FALSE,
  TK_CHAR,
  TK_INT,
  TK_BYTE,
  TK_FLOAT,
  TK_DOUBLE,
  TK_MINUS,
  TK_BYTES,
  TK_STRING,
  TK_UNDERSCORE,
  TK_LIDENT,
  TK_PACKAGE_NAME,
  TK_UIDENT,
  TK_LPAREN,
  TK_LBRACKET,
  TK_LBRACE,
]
