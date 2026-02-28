///|
pub enum LexicalError {
  IllegalCharacter(Char)
  UnterminatedString
  UnterminatedStringInVariableInterploation
  InterpInvalidMultilineString
  InterpInvalidComment
  InterpInvalidAttribute
  InterpMissingExpression
  InvalidDotInt(String)
  InvalidByteLiteral(String)
  Reserved_keyword(String)
}

///|
pub impl Show for LexicalError with output(self, logger) {
  logger.write_string(
    match self {
      InvalidDotInt(s) => "invalid byte literal: \{s}"
      InterpMissingExpression => "missing expression in string interpolation"
      InterpInvalidAttribute => "invalid attribute in string interpolation"
      InterpInvalidComment => "invalid comment in string interpolation"
      InterpInvalidMultilineString =>
        "invalid multiline string in string interpolation"
      UnterminatedStringInVariableInterploation =>
        "(unterminated string in variable interpolation)"
      UnterminatedString => "(unterminated string)"
      IllegalCharacter(c) => "unrecognized character u32: \{c}"
      InvalidByteLiteral(s) => "invalid byte literal: \{s}"
      Reserved_keyword(s) => "reserved keyword: \{s}"
    },
  )
}
