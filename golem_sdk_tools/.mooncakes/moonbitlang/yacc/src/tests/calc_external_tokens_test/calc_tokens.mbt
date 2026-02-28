pub(all) enum Token {
  NUMBER(Int)
  PLUS
  MINUS
  STAR
  LPAREN
  RPAREN
} derive(Show)

pub fn Token::kind(self : Token) -> TokenKind {
  match self {
    NUMBER(_) => TK_NUMBER
    PLUS => TK_PLUS
    MINUS => TK_MINUS
    STAR => TK_STAR
    LPAREN => TK_LPAREN
    RPAREN => TK_RPAREN
  }
}

pub(all) enum TokenKind {
  TK_NUMBER
  TK_PLUS
  TK_MINUS
  TK_STAR
  TK_LPAREN
  TK_RPAREN
}

pub impl Show for TokenKind with output(self, logger) {
  logger.write_string(
    match self {
      TK_NUMBER => "NUMBER"
      TK_PLUS => "\"+\""
      TK_MINUS => "\"-\""
      TK_STAR => "\"*\""
      TK_LPAREN => "\"(\""
      TK_RPAREN => "\")\""
    }
  )
}

