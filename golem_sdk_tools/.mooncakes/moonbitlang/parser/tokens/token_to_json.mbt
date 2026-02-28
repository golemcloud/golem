///|
pub impl ToJson for CommentKind with to_json(self) {
  match self {
    InlineTrailing => "InlineTrailing"
    Ownline(leading_blank_line~, trailing_blank_line~) =>
      [
        "Ownline",
        {
          "leading_blank_line": leading_blank_line,
          "trailing_blank_line": trailing_blank_line,
        },
      ]
  }
}

///|
pub impl ToJson for Comment with to_json(self) {
  {
    "content": self.content,
    "kind": self.kind,
    "consumed_by_docstring": self.consumed_by_docstring,
  }
}

///|
pub impl ToJson for Token with to_json(self) {
  match self {
    TRY_QUESTION => "TRY_QUESTION"
    NORAISE => "NORAISE"
    ENUMVIEW => "ENUMVIEW"
    LETREC => "LETREC"
    AND => "AND"
    SUBERROR => "SUBERROR"
    IS => "IS"
    IN => "IN"
    FOR => "FOR"
    DEFER => "DEFER"
    GUARD => "GUARD"
    LOOP => "LOOP"
    TEST => "TEST"
    ELLIPSIS => "ELLIPSIS"
    RANGE_EXCLUSIVE => "RANGE_EXCLUSIVE"
    RANGE_INCLUSIVE => "RANGE_INCLUSIVE"
    DOTDOT => "DOTDOT"
    RETURN => "RETURN"
    WHILE => "WHILE"
    THIN_ARROW => "THIN_ARROW"
    FAT_ARROW => "FAT_ARROW"
    TYPE => "TYPE"
    MUTABLE => "MUTABLE"
    USING => "USING"
    MATCH => "MATCH"
    CONST => "CONST"
    LET => "LET"
    IF => "IF"
    FN => "FN"
    ELSE => "ELSE"
    PIPE => "PIPE"
    AS => "AS"
    PACKAGE_NAME(str) => ["PACKAGE_NAME", str]
    BARBAR => "BARBAR"
    CARET => "CARET"
    AMPER => "AMPER"
    AMPERAMPER => "AMPERAMPER"
    RBRACE => "RBRACE"
    LBRACE => "LBRACE"
    BAR => "BAR"
    UNDERSCORE => "UNDERSCORE"
    RBRACKET => "RBRACKET"
    PLUS => "PLUS"
    LBRACKET => "LBRACKET"
    SEMI(boolean) => ["SEMI", boolean]
    COLON => "COLON"
    COLONCOLON => "COLONCOLON"
    DOT_LPAREN => "DOT_LPAREN"
    DOT_INT(n) => ["DOT_INT", n]
    DOT_UIDENT(str) => ["DOT_UIDENT", str]
    DOT_LIDENT(str) => ["DOT_LIDENT", str]
    EXCLAMATION => "EXCLAMATION"
    QUESTION => "QUESTION"
    MINUS => "MINUS"
    COMMA => "COMMA"
    RPAREN => "RPAREN"
    LPAREN => "LPAREN"
    EQUAL => "EQUAL"
    FNALIAS => "FNALIAS"
    TRAITALIAS => "TRAITALIAS"
    TYPEALIAS => "TYPEALIAS"
    ASYNC => "ASYNC"
    CATCH => "CATCH"
    TRY => "TRY"
    THROW => "THROW"
    RAISE => "RAISE"
    WITH => "WITH"
    IMPL => "IMPL"
    DERIVE => "DERIVE"
    TRAIT => "TRAIT"
    ENUM => "ENUM"
    STRUCT => "STRUCT"
    CONTINUE => "CONTINUE"
    BREAK => "BREAK"
    EXTERN => "EXTERN"
    IMPORT => "IMPORT"
    READONLY => "READONLY"
    PRIV => "PRIV"
    PUB => "PUB"
    TRUE => "TRUE"
    FALSE => "FALSE"
    EOF => "EOF"
    AUGMENTED_ASSIGNMENT(str) => ["AUGMENTED_ASSIGNMENT", str]
    INFIX4(str) => ["INFIX4", str]
    INFIX3(str) => ["INFIX3", str]
    INFIX2(str) => ["INFIX2", str]
    INFIX1(str) => ["INFIX1", str]
    NEWLINE => "NEWLINE"
    COMMENT(comment) => ["COMMENT", comment]
    POST_LABEL(str) => ["POST_LABEL", str]
    UIDENT(str) => ["UIDENT", str]
    LIDENT(str) => ["LIDENT", str]
    ATTRIBUTE((id, dot_id, raw_payload)) =>
      [
        "ATTRIBUTE",
        { "id": id, "dot_id": dot_id.unwrap_or(""), "raw_payload": raw_payload },
      ]
    INTERP(interp) => ["INTERP", interp.to_string_repr()]
    MULTILINE_INTERP(interp) => ["MULTILINE_INTERP", interp.to_string_repr()]
    MULTILINE_STRING(str) => ["MULTILINE_STRING", str]
    STRING(str) => ["STRING", str]
    DOUBLE(str) => ["DOUBLE", str]
    FLOAT(str) => ["FLOAT", str]
    BYTES(str) => ["BYTES", str]
    BYTE(str) => ["BYTE", str]
    INT(str) => ["INT", str]
    CHAR(str) => ["CHAR", str]
    TRY_EXCLAMATION => "TRY_EXCLAMATION"
    LEXMATCH => "LEXMATCH"
    LEXMATCH_QUESTION => "LEXMATCH_QUESTION"
    REGEX_LITERAL(str) => ["REGEX_LITERAL", str]
    REGEX_INTERP(interp) => ["REGEX_INTERP", interp.to_string_repr()]
    DECLARE => "DECLARE"
    RANGE_EXCLUSIVE_REV => "RANGE_EXCLUSIVE_REV"
    RANGE_INCLUSIVE_REV => "RANGE_INCLUSIVE_REV"
    RANGE_LT_INCLUSIVE => "RANGE_LT_INCLUSIVE"
    WHERE => "WHERE"
  }
}
