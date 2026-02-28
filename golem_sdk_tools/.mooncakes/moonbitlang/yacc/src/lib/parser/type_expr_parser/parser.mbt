
pub(all) enum Token {
  EOF
  LPAREN
  RPAREN
  LBRACKET
  RBRACKET
  COMMA
  ARROW
  QUESTION
  IDENT(String)
  PKG_AND_IDENT((String, String))
} derive(Show)

pub fn Token::kind(self : Token) -> TokenKind {
  match self {
    EOF => TK_EOF
    LPAREN => TK_LPAREN
    RPAREN => TK_RPAREN
    LBRACKET => TK_LBRACKET
    RBRACKET => TK_RBRACKET
    COMMA => TK_COMMA
    ARROW => TK_ARROW
    QUESTION => TK_QUESTION
    IDENT(_) => TK_IDENT
    PKG_AND_IDENT(_) => TK_PKG_AND_IDENT
  }
}

pub(all) enum TokenKind {
  TK_EOF
  TK_LPAREN
  TK_RPAREN
  TK_LBRACKET
  TK_RBRACKET
  TK_COMMA
  TK_ARROW
  TK_QUESTION
  TK_IDENT
  TK_PKG_AND_IDENT
}

pub impl Show for TokenKind with output(self, logger) {
  logger.write_string(
    match self {
      TK_EOF => "EOF"
      TK_LPAREN => "\"(\""
      TK_RPAREN => "\")\""
      TK_LBRACKET => "\"[\""
      TK_RBRACKET => "\"]\""
      TK_COMMA => "\",\""
      TK_ARROW => "\"->\""
      TK_QUESTION => "\"?\""
      TK_IDENT => "IDENT"
      TK_PKG_AND_IDENT => "PKG_AND_IDENT"
    }
  )
}

pub suberror ParseError {
  UnexpectedToken(Token, (Int, Int), Array[TokenKind])
  UnexpectedEndOfInput(Int, Array[TokenKind])
} derive(Show)

type YYObj = Error

priv suberror YYObj_Void

priv suberror YYObj_String {
  YYObj_String(String)
}

priv suberror YYObj__ast_TypeExpr {
  YYObj__ast_TypeExpr(@ast.TypeExpr)
}

priv suberror YYObj__list_List__ast_TypeExpr_ {
  YYObj__list_List__ast_TypeExpr_(@list.List[@ast.TypeExpr])
}

priv suberror YYObj__String__String_ {
  YYObj__String__String_((String, String))
}

type YYState = (YYSymbol) -> YYDecision

type YYAction = (Int, ArrayView[(YYObj, Int, Int)]) -> YYObj

priv enum YYDecision {
  Accept
  Shift(YYState)
  Reduce(Int, YYSymbol, YYAction)
  ReduceNoLookahead(Int, YYSymbol, YYAction)
  Error
}

priv enum YYSymbol {
  T_EOF
  T_LPAREN
  T_RPAREN
  T_LBRACKET
  T_RBRACKET
  T_COMMA
  T_ARROW
  T_QUESTION
  T_IDENT
  T_PKG_AND_IDENT
  NT_parse_type_expr
  NT_type_expr
  NT_postfix_type_expr
  NT_basic_type_expr
  NT_nonempty_type_expr_list
  EOI
}

// Workaround for EOI unused warning
fn init {
  match (EOI : YYSymbol) {
    EOI => ()
    _ => ()
  }
}

// file:///./parser.mbty
// 36|    type_expr EOF { $1 }
fn yy_action_0(_last_pos : Int, _args : ArrayView[(YYObj, Int, Int)]) -> YYObj {
  guard _args[0].0 is YYObj__ast_TypeExpr(_dollar1)
  YYObj__ast_TypeExpr({();  _dollar1 })
}

// file:///./parser.mbty
// 56|    PKG_AND_IDENT "[" nonempty_type_expr_list "]" { Constr(pkg=Some($1.0), $1.1, $3.to_array()) }
fn yy_action_1(_last_pos : Int, _args : ArrayView[(YYObj, Int, Int)]) -> YYObj {
  guard _args[0].0 is YYObj__String__String_(_dollar1)
  guard _args[2].0 is YYObj__list_List__ast_TypeExpr_(_dollar3)
  YYObj__ast_TypeExpr({();  Constr(pkg=Some(_dollar1.0), _dollar1.1, _dollar3.to_array()) })
}

// file:///./parser.mbty
// 41|    "(" ")" "->" type_expr { Arrow([], $4) }
fn yy_action_2(_last_pos : Int, _args : ArrayView[(YYObj, Int, Int)]) -> YYObj {
  guard _args[3].0 is YYObj__ast_TypeExpr(_dollar4)
  YYObj__ast_TypeExpr({();  Arrow([], _dollar4) })
}

// file:///./parser.mbty
// 42|    "(" type_expr ")" "->" type_expr { Arrow([$2], $5) }
fn yy_action_3(_last_pos : Int, _args : ArrayView[(YYObj, Int, Int)]) -> YYObj {
  guard _args[1].0 is YYObj__ast_TypeExpr(_dollar2)
  guard _args[4].0 is YYObj__ast_TypeExpr(_dollar5)
  YYObj__ast_TypeExpr({();  Arrow([_dollar2], _dollar5) })
}

// file:///./parser.mbty
// 58|    "(" type_expr ")" { $2 }
fn yy_action_4(_last_pos : Int, _args : ArrayView[(YYObj, Int, Int)]) -> YYObj {
  guard _args[1].0 is YYObj__ast_TypeExpr(_dollar2)
  YYObj__ast_TypeExpr({();  _dollar2 })
}

// file:///./parser.mbty
// 43|    "(" type_expr "," ")" "->" type_expr { Arrow([$2], $6) }
fn yy_action_5(_last_pos : Int, _args : ArrayView[(YYObj, Int, Int)]) -> YYObj {
  guard _args[1].0 is YYObj__ast_TypeExpr(_dollar2)
  guard _args[5].0 is YYObj__ast_TypeExpr(_dollar6)
  YYObj__ast_TypeExpr({();  Arrow([_dollar2], _dollar6) })
}

// file:///./parser.mbty
// 49|    postfix_type_expr "?" { Option($1) }
fn yy_action_6(_last_pos : Int, _args : ArrayView[(YYObj, Int, Int)]) -> YYObj {
  guard _args[0].0 is YYObj__ast_TypeExpr(_dollar1)
  YYObj__ast_TypeExpr({();  Option(_dollar1) })
}

// file:///./parser.mbty
// 40|    postfix_type_expr { $1 }
fn yy_action_7(_last_pos : Int, _args : ArrayView[(YYObj, Int, Int)]) -> YYObj {
  guard _args[0].0 is YYObj__ast_TypeExpr(_dollar1)
  YYObj__ast_TypeExpr({();  _dollar1 })
}

// file:///./parser.mbty
// 44|    "(" type_expr "," nonempty_type_expr_list ")" "->" type_expr { Arrow(@list.cons($2, $4).to_array(), $7) }
fn yy_action_8(_last_pos : Int, _args : ArrayView[(YYObj, Int, Int)]) -> YYObj {
  guard _args[1].0 is YYObj__ast_TypeExpr(_dollar2)
  guard _args[3].0 is YYObj__list_List__ast_TypeExpr_(_dollar4)
  guard _args[6].0 is YYObj__ast_TypeExpr(_dollar7)
  YYObj__ast_TypeExpr({();  Arrow(@list.cons(_dollar2, _dollar4).to_array(), _dollar7) })
}

// file:///./parser.mbty
// 57|    "(" type_expr "," nonempty_type_expr_list ")" { Tuple(@list.cons($2, $4).to_array()) }
fn yy_action_9(_last_pos : Int, _args : ArrayView[(YYObj, Int, Int)]) -> YYObj {
  guard _args[1].0 is YYObj__ast_TypeExpr(_dollar2)
  guard _args[3].0 is YYObj__list_List__ast_TypeExpr_(_dollar4)
  YYObj__ast_TypeExpr({();  Tuple(@list.cons(_dollar2, _dollar4).to_array()) })
}

// file:///./parser.mbty
// 48|    basic_type_expr { $1 }
fn yy_action_10(_last_pos : Int, _args : ArrayView[(YYObj, Int, Int)]) -> YYObj {
  guard _args[0].0 is YYObj__ast_TypeExpr(_dollar1)
  YYObj__ast_TypeExpr({();  _dollar1 })
}

// file:///./parser.mbty
// 55|    IDENT "[" nonempty_type_expr_list "]" { Constr(pkg=None, $1, $3.to_array()) }
fn yy_action_11(_last_pos : Int, _args : ArrayView[(YYObj, Int, Int)]) -> YYObj {
  guard _args[0].0 is YYObj_String(_dollar1)
  guard _args[2].0 is YYObj__list_List__ast_TypeExpr_(_dollar3)
  YYObj__ast_TypeExpr({();  Constr(pkg=None, _dollar1, _dollar3.to_array()) })
}

// file:///./parser.mbty
// 53|    IDENT { Constr(pkg=None, $1, []) }
fn yy_action_12(_last_pos : Int, _args : ArrayView[(YYObj, Int, Int)]) -> YYObj {
  guard _args[0].0 is YYObj_String(_dollar1)
  YYObj__ast_TypeExpr({();  Constr(pkg=None, _dollar1, []) })
}

// file:///./parser.mbty
// 63|    type_expr=type_expr "," type_expr_list=nonempty_type_expr_list { @list.cons(type_expr, type_expr_list) }
fn yy_action_13(_last_pos : Int, _args : ArrayView[(YYObj, Int, Int)]) -> YYObj {
  guard _args[0].0 is YYObj__ast_TypeExpr(type_expr)
  guard _args[2].0 is YYObj__list_List__ast_TypeExpr_(type_expr_list)
  YYObj__list_List__ast_TypeExpr_({();  @list.cons(type_expr, type_expr_list) })
}

// file:///./parser.mbty
// 62|    type_expr=type_expr { @list.cons(type_expr, @list.empty()) }
fn yy_action_14(_last_pos : Int, _args : ArrayView[(YYObj, Int, Int)]) -> YYObj {
  guard _args[0].0 is YYObj__ast_TypeExpr(type_expr)
  YYObj__list_List__ast_TypeExpr_({();  @list.cons(type_expr, @list.empty()) })
}

// file:///./parser.mbty
// 54|    PKG_AND_IDENT { Constr(pkg=Some($1.0), $1.1, []) }
fn yy_action_15(_last_pos : Int, _args : ArrayView[(YYObj, Int, Int)]) -> YYObj {
  guard _args[0].0 is YYObj__String__String_(_dollar1)
  YYObj__ast_TypeExpr({();  Constr(pkg=Some(_dollar1.0), _dollar1.1, []) })
}

fn yy_input(token : Token, _start_pos : Int, _end_pos : Int) -> (YYSymbol, YYObj) {
  match token {
    EOF => (T_EOF, YYObj_Void)
    LPAREN => (T_LPAREN, YYObj_Void)
    RPAREN => (T_RPAREN, YYObj_Void)
    LBRACKET => (T_LBRACKET, YYObj_Void)
    RBRACKET => (T_RBRACKET, YYObj_Void)
    COMMA => (T_COMMA, YYObj_Void)
    ARROW => (T_ARROW, YYObj_Void)
    QUESTION => (T_QUESTION, YYObj_Void)
    IDENT(data) => (T_IDENT, YYObj_String(data))
    PKG_AND_IDENT(data) => (T_PKG_AND_IDENT, YYObj__String__String_(data))
  }
}

// [0, parse_type_expr → • type_expr EOF, $]
// [1, type_expr → • postfix_type_expr, EOF]
// [2, type_expr → • LPAREN RPAREN ARROW type_expr, EOF]
// [3, type_expr → • LPAREN type_expr RPAREN ARROW type_expr, EOF]
// [4, type_expr → • LPAREN type_expr COMMA RPAREN ARROW type_expr, EOF]
// [5, type_expr → • LPAREN type_expr COMMA nonempty_type_expr_list RPAREN ARROW type_expr, EOF]
// [6, postfix_type_expr → • basic_type_expr, EOF / QUESTION]
// [7, postfix_type_expr → • postfix_type_expr QUESTION, EOF / QUESTION]
// [8, basic_type_expr → • IDENT, EOF / QUESTION]
// [9, basic_type_expr → • PKG_AND_IDENT, EOF / QUESTION]
// [10, basic_type_expr → • IDENT LBRACKET nonempty_type_expr_list RBRACKET, EOF / QUESTION]
// [11, basic_type_expr → • PKG_AND_IDENT LBRACKET nonempty_type_expr_list RBRACKET, EOF / QUESTION]
// [12, basic_type_expr → • LPAREN type_expr COMMA nonempty_type_expr_list RPAREN, EOF / QUESTION]
// [13, basic_type_expr → • LPAREN type_expr RPAREN, EOF / QUESTION]
// [16, parse_type_expr_prime → • parse_type_expr, $]
fn yy_state_0(_lookahead : YYSymbol) -> YYDecision {
  match _lookahead {
    NT_parse_type_expr => Shift(yy_state_1)
    T_PKG_AND_IDENT => Shift(yy_state_2)
    T_IDENT => Shift(yy_state_7)
    NT_basic_type_expr => Shift(yy_state_11)
    T_LPAREN => Shift(yy_state_12)
    NT_postfix_type_expr => Shift(yy_state_19)
    NT_type_expr => Shift(yy_state_32)
    _ => Error
  }
}

// [16, parse_type_expr_prime → parse_type_expr •, $]
fn yy_state_1(_lookahead : YYSymbol) -> YYDecision {
  Accept
}

// [9, basic_type_expr → PKG_AND_IDENT •, EOF / RPAREN / RBRACKET / COMMA / QUESTION]
// [11, basic_type_expr → PKG_AND_IDENT • LBRACKET nonempty_type_expr_list RBRACKET, EOF / RPAREN / RBRACKET / COMMA / QUESTION]
fn yy_state_2(_lookahead : YYSymbol) -> YYDecision {
  match _lookahead {
    T_LBRACKET => Shift(yy_state_3)
    T_EOF | T_RPAREN | T_RBRACKET | T_COMMA | T_QUESTION => Reduce(1, NT_basic_type_expr, yy_action_15)
    _ => Error
  }
}

// [1, type_expr → • postfix_type_expr, RBRACKET / COMMA]
// [2, type_expr → • LPAREN RPAREN ARROW type_expr, RBRACKET / COMMA]
// [3, type_expr → • LPAREN type_expr RPAREN ARROW type_expr, RBRACKET / COMMA]
// [4, type_expr → • LPAREN type_expr COMMA RPAREN ARROW type_expr, RBRACKET / COMMA]
// [5, type_expr → • LPAREN type_expr COMMA nonempty_type_expr_list RPAREN ARROW type_expr, RBRACKET / COMMA]
// [6, postfix_type_expr → • basic_type_expr, RBRACKET / COMMA / QUESTION]
// [7, postfix_type_expr → • postfix_type_expr QUESTION, RBRACKET / COMMA / QUESTION]
// [8, basic_type_expr → • IDENT, RBRACKET / COMMA / QUESTION]
// [9, basic_type_expr → • PKG_AND_IDENT, RBRACKET / COMMA / QUESTION]
// [10, basic_type_expr → • IDENT LBRACKET nonempty_type_expr_list RBRACKET, RBRACKET / COMMA / QUESTION]
// [11, basic_type_expr → • PKG_AND_IDENT LBRACKET nonempty_type_expr_list RBRACKET, RBRACKET / COMMA / QUESTION]
// [11, basic_type_expr → PKG_AND_IDENT LBRACKET • nonempty_type_expr_list RBRACKET, EOF / RPAREN / RBRACKET / COMMA / QUESTION]
// [12, basic_type_expr → • LPAREN type_expr COMMA nonempty_type_expr_list RPAREN, RBRACKET / COMMA / QUESTION]
// [13, basic_type_expr → • LPAREN type_expr RPAREN, RBRACKET / COMMA / QUESTION]
// [14, nonempty_type_expr_list → • type_expr, RBRACKET]
// [15, nonempty_type_expr_list → • type_expr COMMA nonempty_type_expr_list, RBRACKET]
fn yy_state_3(_lookahead : YYSymbol) -> YYDecision {
  match _lookahead {
    T_PKG_AND_IDENT => Shift(yy_state_2)
    NT_type_expr => Shift(yy_state_4)
    T_IDENT => Shift(yy_state_7)
    NT_basic_type_expr => Shift(yy_state_11)
    T_LPAREN => Shift(yy_state_12)
    NT_postfix_type_expr => Shift(yy_state_19)
    NT_nonempty_type_expr_list => Shift(yy_state_30)
    _ => Error
  }
}

// [14, nonempty_type_expr_list → type_expr •, RPAREN / RBRACKET]
// [15, nonempty_type_expr_list → type_expr • COMMA nonempty_type_expr_list, RPAREN / RBRACKET]
fn yy_state_4(_lookahead : YYSymbol) -> YYDecision {
  match _lookahead {
    T_COMMA => Shift(yy_state_5)
    T_RPAREN | T_RBRACKET => Reduce(1, NT_nonempty_type_expr_list, yy_action_14)
    _ => Error
  }
}

// [1, type_expr → • postfix_type_expr, RPAREN / RBRACKET / COMMA]
// [2, type_expr → • LPAREN RPAREN ARROW type_expr, RPAREN / RBRACKET / COMMA]
// [3, type_expr → • LPAREN type_expr RPAREN ARROW type_expr, RPAREN / RBRACKET / COMMA]
// [4, type_expr → • LPAREN type_expr COMMA RPAREN ARROW type_expr, RPAREN / RBRACKET / COMMA]
// [5, type_expr → • LPAREN type_expr COMMA nonempty_type_expr_list RPAREN ARROW type_expr, RPAREN / RBRACKET / COMMA]
// [6, postfix_type_expr → • basic_type_expr, RPAREN / RBRACKET / COMMA / QUESTION]
// [7, postfix_type_expr → • postfix_type_expr QUESTION, RPAREN / RBRACKET / COMMA / QUESTION]
// [8, basic_type_expr → • IDENT, RPAREN / RBRACKET / COMMA / QUESTION]
// [9, basic_type_expr → • PKG_AND_IDENT, RPAREN / RBRACKET / COMMA / QUESTION]
// [10, basic_type_expr → • IDENT LBRACKET nonempty_type_expr_list RBRACKET, RPAREN / RBRACKET / COMMA / QUESTION]
// [11, basic_type_expr → • PKG_AND_IDENT LBRACKET nonempty_type_expr_list RBRACKET, RPAREN / RBRACKET / COMMA / QUESTION]
// [12, basic_type_expr → • LPAREN type_expr COMMA nonempty_type_expr_list RPAREN, RPAREN / RBRACKET / COMMA / QUESTION]
// [13, basic_type_expr → • LPAREN type_expr RPAREN, RPAREN / RBRACKET / COMMA / QUESTION]
// [14, nonempty_type_expr_list → • type_expr, RPAREN / RBRACKET]
// [15, nonempty_type_expr_list → • type_expr COMMA nonempty_type_expr_list, RPAREN / RBRACKET]
// [15, nonempty_type_expr_list → type_expr COMMA • nonempty_type_expr_list, RPAREN / RBRACKET]
fn yy_state_5(_lookahead : YYSymbol) -> YYDecision {
  match _lookahead {
    T_PKG_AND_IDENT => Shift(yy_state_2)
    NT_type_expr => Shift(yy_state_4)
    NT_nonempty_type_expr_list => Shift(yy_state_6)
    T_IDENT => Shift(yy_state_7)
    NT_basic_type_expr => Shift(yy_state_11)
    T_LPAREN => Shift(yy_state_12)
    NT_postfix_type_expr => Shift(yy_state_19)
    _ => Error
  }
}

// [15, nonempty_type_expr_list → type_expr COMMA nonempty_type_expr_list •, RPAREN / RBRACKET]
fn yy_state_6(_lookahead : YYSymbol) -> YYDecision {
  ReduceNoLookahead(3, NT_nonempty_type_expr_list, yy_action_13)
}

// [8, basic_type_expr → IDENT •, EOF / RPAREN / RBRACKET / COMMA / QUESTION]
// [10, basic_type_expr → IDENT • LBRACKET nonempty_type_expr_list RBRACKET, EOF / RPAREN / RBRACKET / COMMA / QUESTION]
fn yy_state_7(_lookahead : YYSymbol) -> YYDecision {
  match _lookahead {
    T_LBRACKET => Shift(yy_state_8)
    T_EOF | T_RPAREN | T_RBRACKET | T_COMMA | T_QUESTION => Reduce(1, NT_basic_type_expr, yy_action_12)
    _ => Error
  }
}

// [1, type_expr → • postfix_type_expr, RBRACKET / COMMA]
// [2, type_expr → • LPAREN RPAREN ARROW type_expr, RBRACKET / COMMA]
// [3, type_expr → • LPAREN type_expr RPAREN ARROW type_expr, RBRACKET / COMMA]
// [4, type_expr → • LPAREN type_expr COMMA RPAREN ARROW type_expr, RBRACKET / COMMA]
// [5, type_expr → • LPAREN type_expr COMMA nonempty_type_expr_list RPAREN ARROW type_expr, RBRACKET / COMMA]
// [6, postfix_type_expr → • basic_type_expr, RBRACKET / COMMA / QUESTION]
// [7, postfix_type_expr → • postfix_type_expr QUESTION, RBRACKET / COMMA / QUESTION]
// [8, basic_type_expr → • IDENT, RBRACKET / COMMA / QUESTION]
// [9, basic_type_expr → • PKG_AND_IDENT, RBRACKET / COMMA / QUESTION]
// [10, basic_type_expr → • IDENT LBRACKET nonempty_type_expr_list RBRACKET, RBRACKET / COMMA / QUESTION]
// [10, basic_type_expr → IDENT LBRACKET • nonempty_type_expr_list RBRACKET, EOF / RPAREN / RBRACKET / COMMA / QUESTION]
// [11, basic_type_expr → • PKG_AND_IDENT LBRACKET nonempty_type_expr_list RBRACKET, RBRACKET / COMMA / QUESTION]
// [12, basic_type_expr → • LPAREN type_expr COMMA nonempty_type_expr_list RPAREN, RBRACKET / COMMA / QUESTION]
// [13, basic_type_expr → • LPAREN type_expr RPAREN, RBRACKET / COMMA / QUESTION]
// [14, nonempty_type_expr_list → • type_expr, RBRACKET]
// [15, nonempty_type_expr_list → • type_expr COMMA nonempty_type_expr_list, RBRACKET]
fn yy_state_8(_lookahead : YYSymbol) -> YYDecision {
  match _lookahead {
    T_PKG_AND_IDENT => Shift(yy_state_2)
    NT_type_expr => Shift(yy_state_4)
    T_IDENT => Shift(yy_state_7)
    NT_nonempty_type_expr_list => Shift(yy_state_9)
    NT_basic_type_expr => Shift(yy_state_11)
    T_LPAREN => Shift(yy_state_12)
    NT_postfix_type_expr => Shift(yy_state_19)
    _ => Error
  }
}

// [10, basic_type_expr → IDENT LBRACKET nonempty_type_expr_list • RBRACKET, EOF / RPAREN / RBRACKET / COMMA / QUESTION]
fn yy_state_9(_lookahead : YYSymbol) -> YYDecision {
  match _lookahead {
    T_RBRACKET => Shift(yy_state_10)
    _ => Error
  }
}

// [10, basic_type_expr → IDENT LBRACKET nonempty_type_expr_list RBRACKET •, EOF / RPAREN / RBRACKET / COMMA / QUESTION]
fn yy_state_10(_lookahead : YYSymbol) -> YYDecision {
  ReduceNoLookahead(4, NT_basic_type_expr, yy_action_11)
}

// [6, postfix_type_expr → basic_type_expr •, EOF / RPAREN / RBRACKET / COMMA / QUESTION]
fn yy_state_11(_lookahead : YYSymbol) -> YYDecision {
  ReduceNoLookahead(1, NT_postfix_type_expr, yy_action_10)
}

// [1, type_expr → • postfix_type_expr, RPAREN / COMMA]
// [2, type_expr → • LPAREN RPAREN ARROW type_expr, RPAREN / COMMA]
// [2, type_expr → LPAREN • RPAREN ARROW type_expr, EOF / RPAREN / RBRACKET / COMMA]
// [3, type_expr → • LPAREN type_expr RPAREN ARROW type_expr, RPAREN / COMMA]
// [3, type_expr → LPAREN • type_expr RPAREN ARROW type_expr, EOF / RPAREN / RBRACKET / COMMA]
// [4, type_expr → • LPAREN type_expr COMMA RPAREN ARROW type_expr, RPAREN / COMMA]
// [4, type_expr → LPAREN • type_expr COMMA RPAREN ARROW type_expr, EOF / RPAREN / RBRACKET / COMMA]
// [5, type_expr → • LPAREN type_expr COMMA nonempty_type_expr_list RPAREN ARROW type_expr, RPAREN / COMMA]
// [5, type_expr → LPAREN • type_expr COMMA nonempty_type_expr_list RPAREN ARROW type_expr, EOF / RPAREN / RBRACKET / COMMA]
// [6, postfix_type_expr → • basic_type_expr, RPAREN / COMMA / QUESTION]
// [7, postfix_type_expr → • postfix_type_expr QUESTION, RPAREN / COMMA / QUESTION]
// [8, basic_type_expr → • IDENT, RPAREN / COMMA / QUESTION]
// [9, basic_type_expr → • PKG_AND_IDENT, RPAREN / COMMA / QUESTION]
// [10, basic_type_expr → • IDENT LBRACKET nonempty_type_expr_list RBRACKET, RPAREN / COMMA / QUESTION]
// [11, basic_type_expr → • PKG_AND_IDENT LBRACKET nonempty_type_expr_list RBRACKET, RPAREN / COMMA / QUESTION]
// [12, basic_type_expr → • LPAREN type_expr COMMA nonempty_type_expr_list RPAREN, RPAREN / COMMA / QUESTION]
// [12, basic_type_expr → LPAREN • type_expr COMMA nonempty_type_expr_list RPAREN, EOF / RPAREN / RBRACKET / COMMA / QUESTION]
// [13, basic_type_expr → • LPAREN type_expr RPAREN, RPAREN / COMMA / QUESTION]
// [13, basic_type_expr → LPAREN • type_expr RPAREN, EOF / RPAREN / RBRACKET / COMMA / QUESTION]
fn yy_state_12(_lookahead : YYSymbol) -> YYDecision {
  match _lookahead {
    T_PKG_AND_IDENT => Shift(yy_state_2)
    T_IDENT => Shift(yy_state_7)
    NT_basic_type_expr => Shift(yy_state_11)
    T_LPAREN => Shift(yy_state_12)
    NT_type_expr => Shift(yy_state_13)
    NT_postfix_type_expr => Shift(yy_state_19)
    T_RPAREN => Shift(yy_state_27)
    _ => Error
  }
}

// [3, type_expr → LPAREN type_expr • RPAREN ARROW type_expr, EOF / RPAREN / RBRACKET / COMMA]
// [4, type_expr → LPAREN type_expr • COMMA RPAREN ARROW type_expr, EOF / RPAREN / RBRACKET / COMMA]
// [5, type_expr → LPAREN type_expr • COMMA nonempty_type_expr_list RPAREN ARROW type_expr, EOF / RPAREN / RBRACKET / COMMA]
// [12, basic_type_expr → LPAREN type_expr • COMMA nonempty_type_expr_list RPAREN, EOF / RPAREN / RBRACKET / COMMA / QUESTION]
// [13, basic_type_expr → LPAREN type_expr • RPAREN, EOF / RPAREN / RBRACKET / COMMA / QUESTION]
fn yy_state_13(_lookahead : YYSymbol) -> YYDecision {
  match _lookahead {
    T_COMMA => Shift(yy_state_14)
    T_RPAREN => Shift(yy_state_24)
    _ => Error
  }
}

// [1, type_expr → • postfix_type_expr, RPAREN / COMMA]
// [2, type_expr → • LPAREN RPAREN ARROW type_expr, RPAREN / COMMA]
// [3, type_expr → • LPAREN type_expr RPAREN ARROW type_expr, RPAREN / COMMA]
// [4, type_expr → • LPAREN type_expr COMMA RPAREN ARROW type_expr, RPAREN / COMMA]
// [4, type_expr → LPAREN type_expr COMMA • RPAREN ARROW type_expr, EOF / RPAREN / RBRACKET / COMMA]
// [5, type_expr → • LPAREN type_expr COMMA nonempty_type_expr_list RPAREN ARROW type_expr, RPAREN / COMMA]
// [5, type_expr → LPAREN type_expr COMMA • nonempty_type_expr_list RPAREN ARROW type_expr, EOF / RPAREN / RBRACKET / COMMA]
// [6, postfix_type_expr → • basic_type_expr, RPAREN / COMMA / QUESTION]
// [7, postfix_type_expr → • postfix_type_expr QUESTION, RPAREN / COMMA / QUESTION]
// [8, basic_type_expr → • IDENT, RPAREN / COMMA / QUESTION]
// [9, basic_type_expr → • PKG_AND_IDENT, RPAREN / COMMA / QUESTION]
// [10, basic_type_expr → • IDENT LBRACKET nonempty_type_expr_list RBRACKET, RPAREN / COMMA / QUESTION]
// [11, basic_type_expr → • PKG_AND_IDENT LBRACKET nonempty_type_expr_list RBRACKET, RPAREN / COMMA / QUESTION]
// [12, basic_type_expr → • LPAREN type_expr COMMA nonempty_type_expr_list RPAREN, RPAREN / COMMA / QUESTION]
// [12, basic_type_expr → LPAREN type_expr COMMA • nonempty_type_expr_list RPAREN, EOF / RPAREN / RBRACKET / COMMA / QUESTION]
// [13, basic_type_expr → • LPAREN type_expr RPAREN, RPAREN / COMMA / QUESTION]
// [14, nonempty_type_expr_list → • type_expr, RPAREN]
// [15, nonempty_type_expr_list → • type_expr COMMA nonempty_type_expr_list, RPAREN]
fn yy_state_14(_lookahead : YYSymbol) -> YYDecision {
  match _lookahead {
    T_PKG_AND_IDENT => Shift(yy_state_2)
    NT_type_expr => Shift(yy_state_4)
    T_IDENT => Shift(yy_state_7)
    NT_basic_type_expr => Shift(yy_state_11)
    T_LPAREN => Shift(yy_state_12)
    NT_nonempty_type_expr_list => Shift(yy_state_15)
    NT_postfix_type_expr => Shift(yy_state_19)
    T_RPAREN => Shift(yy_state_21)
    _ => Error
  }
}

// [5, type_expr → LPAREN type_expr COMMA nonempty_type_expr_list • RPAREN ARROW type_expr, EOF / RPAREN / RBRACKET / COMMA]
// [12, basic_type_expr → LPAREN type_expr COMMA nonempty_type_expr_list • RPAREN, EOF / RPAREN / RBRACKET / COMMA / QUESTION]
fn yy_state_15(_lookahead : YYSymbol) -> YYDecision {
  match _lookahead {
    T_RPAREN => Shift(yy_state_16)
    _ => Error
  }
}

// [5, type_expr → LPAREN type_expr COMMA nonempty_type_expr_list RPAREN • ARROW type_expr, EOF / RPAREN / RBRACKET / COMMA]
// [12, basic_type_expr → LPAREN type_expr COMMA nonempty_type_expr_list RPAREN •, EOF / RPAREN / RBRACKET / COMMA / QUESTION]
fn yy_state_16(_lookahead : YYSymbol) -> YYDecision {
  match _lookahead {
    T_ARROW => Shift(yy_state_17)
    T_EOF | T_RPAREN | T_RBRACKET | T_COMMA | T_QUESTION => Reduce(5, NT_basic_type_expr, yy_action_9)
    _ => Error
  }
}

// [1, type_expr → • postfix_type_expr, EOF / RPAREN / RBRACKET / COMMA]
// [2, type_expr → • LPAREN RPAREN ARROW type_expr, EOF / RPAREN / RBRACKET / COMMA]
// [3, type_expr → • LPAREN type_expr RPAREN ARROW type_expr, EOF / RPAREN / RBRACKET / COMMA]
// [4, type_expr → • LPAREN type_expr COMMA RPAREN ARROW type_expr, EOF / RPAREN / RBRACKET / COMMA]
// [5, type_expr → • LPAREN type_expr COMMA nonempty_type_expr_list RPAREN ARROW type_expr, EOF / RPAREN / RBRACKET / COMMA]
// [5, type_expr → LPAREN type_expr COMMA nonempty_type_expr_list RPAREN ARROW • type_expr, EOF / RPAREN / RBRACKET / COMMA]
// [6, postfix_type_expr → • basic_type_expr, EOF / RPAREN / RBRACKET / COMMA / QUESTION]
// [7, postfix_type_expr → • postfix_type_expr QUESTION, EOF / RPAREN / RBRACKET / COMMA / QUESTION]
// [8, basic_type_expr → • IDENT, EOF / RPAREN / RBRACKET / COMMA / QUESTION]
// [9, basic_type_expr → • PKG_AND_IDENT, EOF / RPAREN / RBRACKET / COMMA / QUESTION]
// [10, basic_type_expr → • IDENT LBRACKET nonempty_type_expr_list RBRACKET, EOF / RPAREN / RBRACKET / COMMA / QUESTION]
// [11, basic_type_expr → • PKG_AND_IDENT LBRACKET nonempty_type_expr_list RBRACKET, EOF / RPAREN / RBRACKET / COMMA / QUESTION]
// [12, basic_type_expr → • LPAREN type_expr COMMA nonempty_type_expr_list RPAREN, EOF / RPAREN / RBRACKET / COMMA / QUESTION]
// [13, basic_type_expr → • LPAREN type_expr RPAREN, EOF / RPAREN / RBRACKET / COMMA / QUESTION]
fn yy_state_17(_lookahead : YYSymbol) -> YYDecision {
  match _lookahead {
    T_PKG_AND_IDENT => Shift(yy_state_2)
    T_IDENT => Shift(yy_state_7)
    NT_basic_type_expr => Shift(yy_state_11)
    T_LPAREN => Shift(yy_state_12)
    NT_type_expr => Shift(yy_state_18)
    NT_postfix_type_expr => Shift(yy_state_19)
    _ => Error
  }
}

// [5, type_expr → LPAREN type_expr COMMA nonempty_type_expr_list RPAREN ARROW type_expr •, EOF / RPAREN / RBRACKET / COMMA]
fn yy_state_18(_lookahead : YYSymbol) -> YYDecision {
  ReduceNoLookahead(7, NT_type_expr, yy_action_8)
}

// [1, type_expr → postfix_type_expr •, EOF / RPAREN / RBRACKET / COMMA]
// [7, postfix_type_expr → postfix_type_expr • QUESTION, EOF / RPAREN / RBRACKET / COMMA / QUESTION]
fn yy_state_19(_lookahead : YYSymbol) -> YYDecision {
  match _lookahead {
    T_QUESTION => Shift(yy_state_20)
    T_EOF | T_RPAREN | T_RBRACKET | T_COMMA => Reduce(1, NT_type_expr, yy_action_7)
    _ => Error
  }
}

// [7, postfix_type_expr → postfix_type_expr QUESTION •, EOF / RPAREN / RBRACKET / COMMA / QUESTION]
fn yy_state_20(_lookahead : YYSymbol) -> YYDecision {
  ReduceNoLookahead(2, NT_postfix_type_expr, yy_action_6)
}

// [4, type_expr → LPAREN type_expr COMMA RPAREN • ARROW type_expr, EOF / RPAREN / RBRACKET / COMMA]
fn yy_state_21(_lookahead : YYSymbol) -> YYDecision {
  match _lookahead {
    T_ARROW => Shift(yy_state_22)
    _ => Error
  }
}

// [1, type_expr → • postfix_type_expr, EOF / RPAREN / RBRACKET / COMMA]
// [2, type_expr → • LPAREN RPAREN ARROW type_expr, EOF / RPAREN / RBRACKET / COMMA]
// [3, type_expr → • LPAREN type_expr RPAREN ARROW type_expr, EOF / RPAREN / RBRACKET / COMMA]
// [4, type_expr → • LPAREN type_expr COMMA RPAREN ARROW type_expr, EOF / RPAREN / RBRACKET / COMMA]
// [4, type_expr → LPAREN type_expr COMMA RPAREN ARROW • type_expr, EOF / RPAREN / RBRACKET / COMMA]
// [5, type_expr → • LPAREN type_expr COMMA nonempty_type_expr_list RPAREN ARROW type_expr, EOF / RPAREN / RBRACKET / COMMA]
// [6, postfix_type_expr → • basic_type_expr, EOF / RPAREN / RBRACKET / COMMA / QUESTION]
// [7, postfix_type_expr → • postfix_type_expr QUESTION, EOF / RPAREN / RBRACKET / COMMA / QUESTION]
// [8, basic_type_expr → • IDENT, EOF / RPAREN / RBRACKET / COMMA / QUESTION]
// [9, basic_type_expr → • PKG_AND_IDENT, EOF / RPAREN / RBRACKET / COMMA / QUESTION]
// [10, basic_type_expr → • IDENT LBRACKET nonempty_type_expr_list RBRACKET, EOF / RPAREN / RBRACKET / COMMA / QUESTION]
// [11, basic_type_expr → • PKG_AND_IDENT LBRACKET nonempty_type_expr_list RBRACKET, EOF / RPAREN / RBRACKET / COMMA / QUESTION]
// [12, basic_type_expr → • LPAREN type_expr COMMA nonempty_type_expr_list RPAREN, EOF / RPAREN / RBRACKET / COMMA / QUESTION]
// [13, basic_type_expr → • LPAREN type_expr RPAREN, EOF / RPAREN / RBRACKET / COMMA / QUESTION]
fn yy_state_22(_lookahead : YYSymbol) -> YYDecision {
  match _lookahead {
    T_PKG_AND_IDENT => Shift(yy_state_2)
    T_IDENT => Shift(yy_state_7)
    NT_basic_type_expr => Shift(yy_state_11)
    T_LPAREN => Shift(yy_state_12)
    NT_postfix_type_expr => Shift(yy_state_19)
    NT_type_expr => Shift(yy_state_23)
    _ => Error
  }
}

// [4, type_expr → LPAREN type_expr COMMA RPAREN ARROW type_expr •, EOF / RPAREN / RBRACKET / COMMA]
fn yy_state_23(_lookahead : YYSymbol) -> YYDecision {
  ReduceNoLookahead(6, NT_type_expr, yy_action_5)
}

// [3, type_expr → LPAREN type_expr RPAREN • ARROW type_expr, EOF / RPAREN / RBRACKET / COMMA]
// [13, basic_type_expr → LPAREN type_expr RPAREN •, EOF / RPAREN / RBRACKET / COMMA / QUESTION]
fn yy_state_24(_lookahead : YYSymbol) -> YYDecision {
  match _lookahead {
    T_ARROW => Shift(yy_state_25)
    T_EOF | T_RPAREN | T_RBRACKET | T_COMMA | T_QUESTION => Reduce(3, NT_basic_type_expr, yy_action_4)
    _ => Error
  }
}

// [1, type_expr → • postfix_type_expr, EOF / RPAREN / RBRACKET / COMMA]
// [2, type_expr → • LPAREN RPAREN ARROW type_expr, EOF / RPAREN / RBRACKET / COMMA]
// [3, type_expr → • LPAREN type_expr RPAREN ARROW type_expr, EOF / RPAREN / RBRACKET / COMMA]
// [3, type_expr → LPAREN type_expr RPAREN ARROW • type_expr, EOF / RPAREN / RBRACKET / COMMA]
// [4, type_expr → • LPAREN type_expr COMMA RPAREN ARROW type_expr, EOF / RPAREN / RBRACKET / COMMA]
// [5, type_expr → • LPAREN type_expr COMMA nonempty_type_expr_list RPAREN ARROW type_expr, EOF / RPAREN / RBRACKET / COMMA]
// [6, postfix_type_expr → • basic_type_expr, EOF / RPAREN / RBRACKET / COMMA / QUESTION]
// [7, postfix_type_expr → • postfix_type_expr QUESTION, EOF / RPAREN / RBRACKET / COMMA / QUESTION]
// [8, basic_type_expr → • IDENT, EOF / RPAREN / RBRACKET / COMMA / QUESTION]
// [9, basic_type_expr → • PKG_AND_IDENT, EOF / RPAREN / RBRACKET / COMMA / QUESTION]
// [10, basic_type_expr → • IDENT LBRACKET nonempty_type_expr_list RBRACKET, EOF / RPAREN / RBRACKET / COMMA / QUESTION]
// [11, basic_type_expr → • PKG_AND_IDENT LBRACKET nonempty_type_expr_list RBRACKET, EOF / RPAREN / RBRACKET / COMMA / QUESTION]
// [12, basic_type_expr → • LPAREN type_expr COMMA nonempty_type_expr_list RPAREN, EOF / RPAREN / RBRACKET / COMMA / QUESTION]
// [13, basic_type_expr → • LPAREN type_expr RPAREN, EOF / RPAREN / RBRACKET / COMMA / QUESTION]
fn yy_state_25(_lookahead : YYSymbol) -> YYDecision {
  match _lookahead {
    T_PKG_AND_IDENT => Shift(yy_state_2)
    T_IDENT => Shift(yy_state_7)
    NT_basic_type_expr => Shift(yy_state_11)
    T_LPAREN => Shift(yy_state_12)
    NT_postfix_type_expr => Shift(yy_state_19)
    NT_type_expr => Shift(yy_state_26)
    _ => Error
  }
}

// [3, type_expr → LPAREN type_expr RPAREN ARROW type_expr •, EOF / RPAREN / RBRACKET / COMMA]
fn yy_state_26(_lookahead : YYSymbol) -> YYDecision {
  ReduceNoLookahead(5, NT_type_expr, yy_action_3)
}

// [2, type_expr → LPAREN RPAREN • ARROW type_expr, EOF / RPAREN / RBRACKET / COMMA]
fn yy_state_27(_lookahead : YYSymbol) -> YYDecision {
  match _lookahead {
    T_ARROW => Shift(yy_state_28)
    _ => Error
  }
}

// [1, type_expr → • postfix_type_expr, EOF / RPAREN / RBRACKET / COMMA]
// [2, type_expr → • LPAREN RPAREN ARROW type_expr, EOF / RPAREN / RBRACKET / COMMA]
// [2, type_expr → LPAREN RPAREN ARROW • type_expr, EOF / RPAREN / RBRACKET / COMMA]
// [3, type_expr → • LPAREN type_expr RPAREN ARROW type_expr, EOF / RPAREN / RBRACKET / COMMA]
// [4, type_expr → • LPAREN type_expr COMMA RPAREN ARROW type_expr, EOF / RPAREN / RBRACKET / COMMA]
// [5, type_expr → • LPAREN type_expr COMMA nonempty_type_expr_list RPAREN ARROW type_expr, EOF / RPAREN / RBRACKET / COMMA]
// [6, postfix_type_expr → • basic_type_expr, EOF / RPAREN / RBRACKET / COMMA / QUESTION]
// [7, postfix_type_expr → • postfix_type_expr QUESTION, EOF / RPAREN / RBRACKET / COMMA / QUESTION]
// [8, basic_type_expr → • IDENT, EOF / RPAREN / RBRACKET / COMMA / QUESTION]
// [9, basic_type_expr → • PKG_AND_IDENT, EOF / RPAREN / RBRACKET / COMMA / QUESTION]
// [10, basic_type_expr → • IDENT LBRACKET nonempty_type_expr_list RBRACKET, EOF / RPAREN / RBRACKET / COMMA / QUESTION]
// [11, basic_type_expr → • PKG_AND_IDENT LBRACKET nonempty_type_expr_list RBRACKET, EOF / RPAREN / RBRACKET / COMMA / QUESTION]
// [12, basic_type_expr → • LPAREN type_expr COMMA nonempty_type_expr_list RPAREN, EOF / RPAREN / RBRACKET / COMMA / QUESTION]
// [13, basic_type_expr → • LPAREN type_expr RPAREN, EOF / RPAREN / RBRACKET / COMMA / QUESTION]
fn yy_state_28(_lookahead : YYSymbol) -> YYDecision {
  match _lookahead {
    T_PKG_AND_IDENT => Shift(yy_state_2)
    T_IDENT => Shift(yy_state_7)
    NT_basic_type_expr => Shift(yy_state_11)
    T_LPAREN => Shift(yy_state_12)
    NT_postfix_type_expr => Shift(yy_state_19)
    NT_type_expr => Shift(yy_state_29)
    _ => Error
  }
}

// [2, type_expr → LPAREN RPAREN ARROW type_expr •, EOF / RPAREN / RBRACKET / COMMA]
fn yy_state_29(_lookahead : YYSymbol) -> YYDecision {
  ReduceNoLookahead(4, NT_type_expr, yy_action_2)
}

// [11, basic_type_expr → PKG_AND_IDENT LBRACKET nonempty_type_expr_list • RBRACKET, EOF / RPAREN / RBRACKET / COMMA / QUESTION]
fn yy_state_30(_lookahead : YYSymbol) -> YYDecision {
  match _lookahead {
    T_RBRACKET => Shift(yy_state_31)
    _ => Error
  }
}

// [11, basic_type_expr → PKG_AND_IDENT LBRACKET nonempty_type_expr_list RBRACKET •, EOF / RPAREN / RBRACKET / COMMA / QUESTION]
fn yy_state_31(_lookahead : YYSymbol) -> YYDecision {
  ReduceNoLookahead(4, NT_basic_type_expr, yy_action_1)
}

// [0, parse_type_expr → type_expr • EOF, $]
fn yy_state_32(_lookahead : YYSymbol) -> YYDecision {
  match _lookahead {
    T_EOF => Shift(yy_state_33)
    _ => Error
  }
}

// [0, parse_type_expr → type_expr EOF •, $]
fn yy_state_33(_lookahead : YYSymbol) -> YYDecision {
  ReduceNoLookahead(2, NT_parse_type_expr, yy_action_0)
}

fn[T] yy_parse(
  tokens : Array[(Token, Int, Int)],
  start : YYState,
  return_ : (YYObj) -> T,
  initial_pos? : Int,
) -> T raise ParseError {
  let mut cursor = 0
  let mut state_stack : @list.List[YYState] = @list.cons(start, @list.empty())
  let data_stack : Array[(YYObj, Int, Int)] = []
  let mut last_pos = initial_pos.unwrap_or(tokens[0].1)
  let mut state = start
  let mut lookahead : Option[(YYSymbol, (YYObj, Int, Int), Token?)] = None
  let mut last_shifted_state_stack = state_stack
  while true {
    let decision = match state(EOI) {
      ReduceNoLookahead(_) | Accept as t => t
      _ => {
        match lookahead {
          Some(la) => state(la.0)
          None => {
            if cursor < tokens.length() {
              let (token, start_pos, end_pos) = tokens[cursor]
              cursor += 1
              let (symbol, data) = yy_input(token, start_pos, end_pos)
              lookahead = Some((symbol, (data, start_pos, end_pos), Some(token)))
              state(symbol)
            } else {
              lookahead = Some((EOI, (YYObj_Void, last_pos, last_pos), None))
              state(EOI)
            }
          }
        }
      }
    }
    match decision {
      Accept => return return_(data_stack.unsafe_pop().0)
      Shift(next_state) => {
        guard lookahead is Some(la)
        data_stack.push(la.1)
        state_stack = @list.cons(next_state, state_stack)
        last_shifted_state_stack = state_stack
        state = next_state
        last_pos = la.1.2
        lookahead = None
      }
      Reduce(count, symbol, action)
      | ReduceNoLookahead(count, symbol, action) => {
        loop (count, symbol, action) {
          _ => {
            let args = data_stack[data_stack.length() - count:]
            let data = action(last_pos, args)
            let (start_pos, end_pos) = if args.length() == 0 {
              (last_pos, last_pos)
            } else {
              (args[0].1, args[args.length() - 1].2)
            }
            for i in 0..<count {
              ignore(data_stack.unsafe_pop())
              state_stack = state_stack.unsafe_tail()
            }
            state = state_stack.unsafe_head()
            data_stack.push((data, start_pos, end_pos))
            match state(symbol) {
              Accept => return return_(data_stack.unsafe_pop().0)
              Shift(next_state) => {
                state_stack = @list.cons(next_state, state_stack)
                state = next_state
              }
              Reduce(count, symbol, action)
              | ReduceNoLookahead(count, symbol, action) => continue (count, symbol, action)
              _ => panic()
            }
          }
        }
      }
      Error => {
        let (_, (_, start_pos, end_pos), token) = lookahead.unwrap()
        error(last_shifted_state_stack, token, (start_pos, end_pos))
      }
    }
  }
  panic()
}

fn error(stack : @list.List[YYState], token : Token?, loc : (Int, Int)) -> Unit raise ParseError {
  let expected = []
  fn try_add(symbol : YYSymbol, kind : TokenKind) {
    fn go(stack : @list.List[YYState]) {
      match stack {
        Empty => ()
        More(state, ..) => {
          match state(symbol) {
            Accept | Shift(_) => expected.push(kind)
            Reduce(count, symbol, _) | ReduceNoLookahead(count, symbol, _) => {
              fn inner_go(stack : @list.List[YYState], count, symbol) {
                let stack = stack.drop(count)
                guard stack is More(state, ..)
                match state(symbol) {
                  Shift(state) => go(@list.cons(state, stack))
                  Reduce(count, symbol, _) | ReduceNoLookahead(count, symbol, _) => inner_go(stack, count, symbol)
                  _ => panic()
                }
              }
              inner_go(stack, count, symbol)
            }
            Error => ()
          }
        }
      }
    }
    go(stack)
  }
  for term in ([(T_EOF, TK_EOF), (T_LPAREN, TK_LPAREN), (T_RPAREN, TK_RPAREN), (T_LBRACKET, TK_LBRACKET), (T_RBRACKET, TK_RBRACKET), (T_COMMA, TK_COMMA), (T_ARROW, TK_ARROW), (T_QUESTION, TK_QUESTION), (T_IDENT, TK_IDENT), (T_PKG_AND_IDENT, TK_PKG_AND_IDENT)] : Array[(YYSymbol, TokenKind)]) {
    try_add(term.0, term.1)
  }
  match token {
    None => raise UnexpectedEndOfInput(loc.1, expected)
    Some(token) => raise UnexpectedToken(token, loc, expected)
  }
}

pub fn parse_type_expr(tokens : Array[(Token, Int, Int)], initial_pos? : Int) -> @ast.TypeExpr raise ParseError {
  yy_parse(
    tokens,
    yy_state_0,
    (it) => {
      guard it is YYObj__ast_TypeExpr(result)
      result
    },
    initial_pos?,
  )
}
