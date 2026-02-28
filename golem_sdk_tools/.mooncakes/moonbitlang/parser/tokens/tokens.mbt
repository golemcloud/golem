///|
pub(all) enum Token {
  CHAR(CharLiteral)
  INT(String)
  BYTE(CharLiteral)
  BYTES(StringLiteral)
  FLOAT(String)
  DOUBLE(String)
  STRING(StringLiteral)
  MULTILINE_STRING(String)
  MULTILINE_INTERP(InterpLiteral)
  INTERP(InterpLiteral)
  REGEX_LITERAL(String)
  REGEX_INTERP(InterpLiteral)
  ATTRIBUTE((String, String?, String))
  LIDENT(String)
  UIDENT(String)
  POST_LABEL(String)
  COMMENT(Comment)
  NEWLINE
  INFIX1(String)
  INFIX2(String)
  INFIX3(String)
  INFIX4(String)
  AUGMENTED_ASSIGNMENT(String)
  EOF
  FALSE
  TRUE
  PUB
  PRIV
  READONLY
  IMPORT
  EXTERN
  BREAK
  CONTINUE
  STRUCT
  ENUM
  TRAIT
  DERIVE
  IMPL
  WITH
  RAISE
  THROW
  TRY
  CATCH
  ASYNC
  TYPEALIAS
  TRAITALIAS
  FNALIAS
  EQUAL
  LPAREN
  RPAREN
  COMMA
  MINUS
  QUESTION
  EXCLAMATION
  DOT_LIDENT(String)
  DOT_UIDENT(String)
  DOT_INT(Int)
  DOT_LPAREN
  COLONCOLON
  COLON
  SEMI(Bool)
  LBRACKET
  PLUS
  RBRACKET
  UNDERSCORE
  BAR
  LBRACE
  RBRACE
  AMPERAMPER
  AMPER
  CARET
  BARBAR
  PACKAGE_NAME(String)
  AS
  PIPE
  ELSE
  FN
  IF
  LET
  CONST
  MATCH
  USING
  MUTABLE
  TYPE
  FAT_ARROW
  THIN_ARROW
  WHILE
  RETURN
  DOTDOT
  RANGE_INCLUSIVE
  RANGE_LT_INCLUSIVE
  RANGE_EXCLUSIVE
  RANGE_INCLUSIVE_REV
  RANGE_EXCLUSIVE_REV
  ELLIPSIS
  TEST
  LOOP
  GUARD
  DEFER
  FOR
  IN
  IS
  SUBERROR
  AND
  LETREC
  ENUMVIEW
  DECLARE
  NORAISE
  WHERE
  TRY_QUESTION
  TRY_EXCLAMATION
  LEXMATCH
  LEXMATCH_QUESTION
} derive(Show)

///|
pub fn Token::kind(self : Token) -> TokenKind {
  match self {
    CHAR(_) => TK_CHAR
    INT(_) => TK_INT
    BYTE(_) => TK_BYTE
    BYTES(_) => TK_BYTES
    FLOAT(_) => TK_FLOAT
    DOUBLE(_) => TK_DOUBLE
    STRING(_) => TK_STRING
    MULTILINE_STRING(_) => TK_MULTILINE_STRING
    MULTILINE_INTERP(_) => TK_MULTILINE_INTERP
    INTERP(_) => TK_INTERP
    REGEX_LITERAL(_) => TK_REGEX_LITERAL
    REGEX_INTERP(_) => TK_REGEX_INTERP
    ATTRIBUTE(_) => TK_ATTRIBUTE
    LIDENT(_) => TK_LIDENT
    UIDENT(_) => TK_UIDENT
    POST_LABEL(_) => TK_POST_LABEL
    COMMENT(_) => TK_COMMENT
    NEWLINE => TK_NEWLINE
    INFIX1(_) => TK_INFIX1
    INFIX2(_) => TK_INFIX2
    INFIX3(_) => TK_INFIX3
    INFIX4(_) => TK_INFIX4
    AUGMENTED_ASSIGNMENT(_) => TK_AUGMENTED_ASSIGNMENT
    EOF => TK_EOF
    FALSE => TK_FALSE
    TRUE => TK_TRUE
    PUB => TK_PUB
    PRIV => TK_PRIV
    READONLY => TK_READONLY
    IMPORT => TK_IMPORT
    EXTERN => TK_EXTERN
    BREAK => TK_BREAK
    CONTINUE => TK_CONTINUE
    STRUCT => TK_STRUCT
    ENUM => TK_ENUM
    TRAIT => TK_TRAIT
    DERIVE => TK_DERIVE
    IMPL => TK_IMPL
    WITH => TK_WITH
    RAISE => TK_RAISE
    THROW => TK_THROW
    TRY => TK_TRY
    CATCH => TK_CATCH
    ASYNC => TK_ASYNC
    TYPEALIAS => TK_TYPEALIAS
    TRAITALIAS => TK_TRAITALIAS
    FNALIAS => TK_FNALIAS
    EQUAL => TK_EQUAL
    LPAREN => TK_LPAREN
    RPAREN => TK_RPAREN
    COMMA => TK_COMMA
    MINUS => TK_MINUS
    QUESTION => TK_QUESTION
    EXCLAMATION => TK_EXCLAMATION
    DOT_LIDENT(_) => TK_DOT_LIDENT
    DOT_UIDENT(_) => TK_DOT_UIDENT
    DOT_INT(_) => TK_DOT_INT
    DOT_LPAREN => TK_DOT_LPAREN
    COLONCOLON => TK_COLONCOLON
    COLON => TK_COLON
    SEMI(_) => TK_SEMI
    LBRACKET => TK_LBRACKET
    PLUS => TK_PLUS
    RBRACKET => TK_RBRACKET
    UNDERSCORE => TK_UNDERSCORE
    BAR => TK_BAR
    LBRACE => TK_LBRACE
    RBRACE => TK_RBRACE
    AMPERAMPER => TK_AMPERAMPER
    AMPER => TK_AMPER
    CARET => TK_CARET
    BARBAR => TK_BARBAR
    PACKAGE_NAME(_) => TK_PACKAGE_NAME
    AS => TK_AS
    PIPE => TK_PIPE
    ELSE => TK_ELSE
    FN => TK_FN
    IF => TK_IF
    LET => TK_LET
    CONST => TK_CONST
    MATCH => TK_MATCH
    USING => TK_USING
    MUTABLE => TK_MUTABLE
    TYPE => TK_TYPE
    FAT_ARROW => TK_FAT_ARROW
    THIN_ARROW => TK_THIN_ARROW
    WHILE => TK_WHILE
    RETURN => TK_RETURN
    DOTDOT => TK_DOTDOT
    RANGE_INCLUSIVE => TK_RANGE_INCLUSIVE
    RANGE_LT_INCLUSIVE => TK_RANGE_LT_INCLUSIVE
    RANGE_EXCLUSIVE => TK_RANGE_EXCLUSIVE
    RANGE_INCLUSIVE_REV => TK_RANGE_INCLUSIVE_REV
    RANGE_EXCLUSIVE_REV => TK_RANGE_EXCLUSIVE_REV
    ELLIPSIS => TK_ELLIPSIS
    TEST => TK_TEST
    LOOP => TK_LOOP
    GUARD => TK_GUARD
    DEFER => TK_DEFER
    FOR => TK_FOR
    IN => TK_IN
    IS => TK_IS
    SUBERROR => TK_SUBERROR
    AND => TK_AND
    LETREC => TK_LETREC
    ENUMVIEW => TK_ENUMVIEW
    DECLARE => TK_DECLARE
    NORAISE => TK_NORAISE
    WHERE => TK_WHERE
    TRY_QUESTION => TK_TRY_QUESTION
    TRY_EXCLAMATION => TK_TRY_EXCLAMATION
    LEXMATCH => TK_LEXMATCH
    LEXMATCH_QUESTION => TK_LEXMATCH_QUESTION
  }
}

///|
pub(all) enum TokenKind {
  TK_CHAR
  TK_INT
  TK_BYTE
  TK_BYTES
  TK_FLOAT
  TK_DOUBLE
  TK_STRING
  TK_MULTILINE_STRING
  TK_MULTILINE_INTERP
  TK_INTERP
  TK_REGEX_LITERAL
  TK_REGEX_INTERP
  TK_ATTRIBUTE
  TK_LIDENT
  TK_UIDENT
  TK_POST_LABEL
  TK_COMMENT
  TK_NEWLINE
  TK_INFIX1
  TK_INFIX2
  TK_INFIX3
  TK_INFIX4
  TK_AUGMENTED_ASSIGNMENT
  TK_EOF
  TK_FALSE
  TK_TRUE
  TK_PUB
  TK_PRIV
  TK_READONLY
  TK_IMPORT
  TK_EXTERN
  TK_BREAK
  TK_CONTINUE
  TK_STRUCT
  TK_ENUM
  TK_TRAIT
  TK_DERIVE
  TK_IMPL
  TK_WITH
  TK_RAISE
  TK_THROW
  TK_TRY
  TK_CATCH
  TK_ASYNC
  TK_TYPEALIAS
  TK_TRAITALIAS
  TK_FNALIAS
  TK_EQUAL
  TK_LPAREN
  TK_RPAREN
  TK_COMMA
  TK_MINUS
  TK_QUESTION
  TK_EXCLAMATION
  TK_DOT_LIDENT
  TK_DOT_UIDENT
  TK_DOT_INT
  TK_DOT_LPAREN
  TK_COLONCOLON
  TK_COLON
  TK_SEMI
  TK_LBRACKET
  TK_PLUS
  TK_RBRACKET
  TK_UNDERSCORE
  TK_BAR
  TK_LBRACE
  TK_RBRACE
  TK_AMPERAMPER
  TK_AMPER
  TK_CARET
  TK_BARBAR
  TK_PACKAGE_NAME
  TK_AS
  TK_PIPE
  TK_ELSE
  TK_FN
  TK_IF
  TK_LET
  TK_CONST
  TK_MATCH
  TK_USING
  TK_MUTABLE
  TK_TYPE
  TK_FAT_ARROW
  TK_THIN_ARROW
  TK_WHILE
  TK_RETURN
  TK_DOTDOT
  TK_RANGE_INCLUSIVE
  TK_RANGE_LT_INCLUSIVE
  TK_RANGE_EXCLUSIVE
  TK_RANGE_INCLUSIVE_REV
  TK_RANGE_EXCLUSIVE_REV
  TK_ELLIPSIS
  TK_TEST
  TK_LOOP
  TK_GUARD
  TK_DEFER
  TK_FOR
  TK_IN
  TK_IS
  TK_SUBERROR
  TK_AND
  TK_LETREC
  TK_ENUMVIEW
  TK_DECLARE
  TK_NORAISE
  TK_WHERE
  TK_TRY_QUESTION
  TK_TRY_EXCLAMATION
  TK_LEXMATCH
  TK_LEXMATCH_QUESTION
} derive(Eq, Hash, Compare)

///|
pub impl Show for TokenKind with output(self, logger) {
  logger.write_string(
    match self {
      TK_CHAR => "CHAR"
      TK_INT => "INT"
      TK_BYTE => "BYTE"
      TK_BYTES => "BYTES"
      TK_FLOAT => "FLOAT"
      TK_DOUBLE => "DOUBLE"
      TK_STRING => "STRING"
      TK_MULTILINE_STRING => "MULTILINE_STRING"
      TK_MULTILINE_INTERP => "MULTILINE_INTERP"
      TK_INTERP => "INTERP"
      TK_REGEX_LITERAL => "REGEX_LITERAL"
      TK_REGEX_INTERP => "REGEX_INTERP"
      TK_ATTRIBUTE => "ATTRIBUTE"
      TK_LIDENT => "LIDENT"
      TK_UIDENT => "UIDENT"
      TK_POST_LABEL => "POST_LABEL"
      TK_COMMENT => "COMMENT"
      TK_NEWLINE => "NEWLINE"
      TK_INFIX1 => "INFIX1"
      TK_INFIX2 => "INFIX2"
      TK_INFIX3 => "INFIX3"
      TK_INFIX4 => "INFIX4"
      TK_AUGMENTED_ASSIGNMENT => "AUGMENTED_ASSIGNMENT"
      TK_EOF => "EOF"
      TK_FALSE => "FALSE"
      TK_TRUE => "TRUE"
      TK_PUB => "\"pub\""
      TK_PRIV => "\"priv\""
      TK_READONLY => "\"readonly\""
      TK_IMPORT => "\"import\""
      TK_EXTERN => "\"extern\""
      TK_BREAK => "\"break\""
      TK_CONTINUE => "\"continue\""
      TK_STRUCT => "\"struct\""
      TK_ENUM => "\"enum\""
      TK_TRAIT => "\"trait\""
      TK_DERIVE => "\"derive\""
      TK_IMPL => "\"impl\""
      TK_WITH => "\"with\""
      TK_RAISE => "\"raise\""
      TK_THROW => "\"throw\""
      TK_TRY => "\"try\""
      TK_CATCH => "\"catch\""
      TK_ASYNC => "\"async\""
      TK_TYPEALIAS => "\"typealias\""
      TK_TRAITALIAS => "\"traitalias\""
      TK_FNALIAS => "\"fnalias\""
      TK_EQUAL => "\"=\""
      TK_LPAREN => "\"(\""
      TK_RPAREN => "\")\""
      TK_COMMA => "\",\""
      TK_MINUS => "\"-\""
      TK_QUESTION => "\"?\""
      TK_EXCLAMATION => "\"!\""
      TK_DOT_LIDENT => "DOT_LIDENT"
      TK_DOT_UIDENT => "DOT_UIDENT"
      TK_DOT_INT => "DOT_INT"
      TK_DOT_LPAREN => "\".(\""
      TK_COLONCOLON => "\"::\""
      TK_COLON => "\":\""
      TK_SEMI => "SEMI"
      TK_LBRACKET => "\"[\""
      TK_PLUS => "\"+\""
      TK_RBRACKET => "\"]\""
      TK_UNDERSCORE => "\"_\""
      TK_BAR => "\"|\""
      TK_LBRACE => "\"{\""
      TK_RBRACE => "\"}\""
      TK_AMPERAMPER => "\"&&\""
      TK_AMPER => "\"&\""
      TK_CARET => "\"^\""
      TK_BARBAR => "\"||\""
      TK_PACKAGE_NAME => "PACKAGE_NAME"
      TK_AS => "\"as\""
      TK_PIPE => "\"|>\""
      TK_ELSE => "\"else\""
      TK_FN => "\"fn\""
      TK_IF => "\"if\""
      TK_LET => "\"let\""
      TK_CONST => "\"const\""
      TK_MATCH => "\"match\""
      TK_USING => "\"using\""
      TK_MUTABLE => "\"mut\""
      TK_TYPE => "\"type\""
      TK_FAT_ARROW => "\"=>\""
      TK_THIN_ARROW => "\"->\""
      TK_WHILE => "\"while\""
      TK_RETURN => "\"return\""
      TK_DOTDOT => "\"..\""
      TK_RANGE_INCLUSIVE => "\"..=\""
      TK_RANGE_LT_INCLUSIVE => "\"..<=\""
      TK_RANGE_EXCLUSIVE => "\"..<\""
      TK_RANGE_INCLUSIVE_REV => "\">=..\""
      TK_RANGE_EXCLUSIVE_REV => "\">..\""
      TK_ELLIPSIS => "\"...\""
      TK_TEST => "\"test\""
      TK_LOOP => "\"loop\""
      TK_GUARD => "\"guard\""
      TK_DEFER => "\"defer\""
      TK_FOR => "\"for\""
      TK_IN => "\"in\""
      TK_IS => "\"is\""
      TK_SUBERROR => "\"suberror\""
      TK_AND => "\"and\""
      TK_LETREC => "\"letrec\""
      TK_ENUMVIEW => "\"enumview\""
      TK_DECLARE => "\"declare\""
      TK_NORAISE => "\"noraise\""
      TK_WHERE => "\"where\""
      TK_TRY_QUESTION => "\"try?\""
      TK_TRY_EXCLAMATION => "\"try!\""
      TK_LEXMATCH => "\"lexmatch\""
      TK_LEXMATCH_QUESTION => "\"lexmatch?\""
    },
  )
}
