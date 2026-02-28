///|
pub suberror ParseError {
  UnexpectedToken(Token, (Position, Position), Array[TokenKind])
  UnexpectedEndOfInput(Position, Array[TokenKind])
} derive(Show)

///|
type YYObj = Error

///|
priv suberror YYObj_Void

///|
priv suberror YYObj_Int {
  YYObj_Int(Int)
}

///|
priv suberror YYObj_Bool {
  YYObj_Bool(Bool)
}

///|
priv suberror YYObj_Expr {
  YYObj_Expr(Expr)
}

///|
priv suberror YYObj_List_Prop_ {
  YYObj_List_Prop_(List[Prop])
}

///|
priv suberror YYObj_Prop {
  YYObj_Prop(Prop)
}

///|
priv suberror YYObj_String {
  YYObj_String(String)
}

///|
priv suberror YYObj_Comment {
  YYObj_Comment(Comment)
}

///|
priv suberror YYObj_CharLiteral {
  YYObj_CharLiteral(CharLiteral)
}

///|
priv suberror YYObj_InterpLiteral {
  YYObj_InterpLiteral(InterpLiteral)
}

///|
priv suberror YYObj_StringLiteral {
  YYObj_StringLiteral(StringLiteral)
}

///|
priv suberror YYObj__String__String___String_ {
  YYObj__String__String___String_((String, String?, String))
}

///|
type YYState = (YYSymbol) -> YYDecision

///|
type YYAction = (Position, ArrayView[(YYObj, Position, Position)]) -> YYObj

///|
priv enum YYDecision {
  Accept
  Shift(YYState)
  Reduce(Int, YYSymbol, YYAction)
  ReduceNoLookahead(Int, YYSymbol, YYAction)
  Error
}

///|
priv enum YYSymbol {
  T_CHAR
  T_INT
  T_BYTE
  T_BYTES
  T_FLOAT
  T_DOUBLE
  T_STRING
  T_MULTILINE_STRING
  T_MULTILINE_INTERP
  T_INTERP
  T_REGEX_LITERAL
  T_REGEX_INTERP
  T_ATTRIBUTE
  T_LIDENT
  T_UIDENT
  T_POST_LABEL
  T_COMMENT
  T_NEWLINE
  T_INFIX1
  T_INFIX2
  T_INFIX3
  T_INFIX4
  T_AUGMENTED_ASSIGNMENT
  T_EOF
  T_FALSE
  T_TRUE
  T_PUB
  T_PRIV
  T_READONLY
  T_IMPORT
  T_EXTERN
  T_BREAK
  T_CONTINUE
  T_STRUCT
  T_ENUM
  T_TRAIT
  T_DERIVE
  T_IMPL
  T_WITH
  T_RAISE
  T_THROW
  T_TRY
  T_CATCH
  T_ASYNC
  T_TYPEALIAS
  T_TRAITALIAS
  T_FNALIAS
  T_EQUAL
  T_LPAREN
  T_RPAREN
  T_COMMA
  T_MINUS
  T_QUESTION
  T_EXCLAMATION
  T_DOT_LIDENT
  T_DOT_UIDENT
  T_DOT_INT
  T_DOT_LPAREN
  T_COLONCOLON
  T_COLON
  T_SEMI
  T_LBRACKET
  T_PLUS
  T_RBRACKET
  T_UNDERSCORE
  T_BAR
  T_LBRACE
  T_RBRACE
  T_AMPERAMPER
  T_AMPER
  T_CARET
  T_BARBAR
  T_PACKAGE_NAME
  T_AS
  T_PIPE
  T_ELSE
  T_FN
  T_IF
  T_LET
  T_CONST
  T_MATCH
  T_USING
  T_MUTABLE
  T_TYPE
  T_FAT_ARROW
  T_THIN_ARROW
  T_WHILE
  T_RETURN
  T_DOTDOT
  T_RANGE_INCLUSIVE
  T_RANGE_LT_INCLUSIVE
  T_RANGE_EXCLUSIVE
  T_RANGE_INCLUSIVE_REV
  T_RANGE_EXCLUSIVE_REV
  T_ELLIPSIS
  T_TEST
  T_LOOP
  T_GUARD
  T_DEFER
  T_FOR
  T_IN
  T_IS
  T_SUBERROR
  T_AND
  T_LETREC
  T_ENUMVIEW
  T_DECLARE
  T_NORAISE
  T_WHERE
  T_TRY_QUESTION
  T_TRY_EXCLAMATION
  T_LEXMATCH
  T_LEXMATCH_QUESTION
  NT_payloads
  NT_property
  NT_expr
  NT_properties
  NT_non_empty_properties
  EOI
}

// Workaround for EOI unused warning

///|
fn init {
  match (EOI : YYSymbol) {
    EOI => ()
    _ => ()
  }
}

// file:///./parser.mbty
// 150|    EOF { @list.empty() }

///|
fn yy_action_0(
  _last_pos : Position,
  _args : ArrayView[(YYObj, Position, Position)],
) -> YYObj {
  YYObj_List_Prop_(
    {
      ()
      @list.empty()
    },
  )
}

// file:///./parser.mbty
// 151|    properties { $1 }

///|
fn yy_action_1(
  _last_pos : Position,
  _args : ArrayView[(YYObj, Position, Position)],
) -> YYObj {
  guard _args[0].0 is YYObj_List_Prop_(_dollar1)
  YYObj_List_Prop_(
    {
      ()
      _dollar1
    },
  )
}

// file:///./parser.mbty
// 167|    "(" ")"                      { @list.empty() }

///|
fn yy_action_2(
  _last_pos : Position,
  _args : ArrayView[(YYObj, Position, Position)],
) -> YYObj {
  YYObj_List_Prop_(
    {
      ()
      @list.empty()
    },
  )
}

// file:///./parser.mbty
// 168|    "(" non_empty_properties ")" { $2 }

///|
fn yy_action_3(
  _last_pos : Position,
  _args : ArrayView[(YYObj, Position, Position)],
) -> YYObj {
  guard _args[1].0 is YYObj_List_Prop_(_dollar2)
  YYObj_List_Prop_(
    {
      ()
      _dollar2
    },
  )
}

// file:///./parser.mbty
// 154|    name=LIDENT "=" expr   { Labeled(name, $3) }

///|
fn yy_action_4(
  _last_pos : Position,
  _args : ArrayView[(YYObj, Position, Position)],
) -> YYObj {
  guard _args[0].0 is YYObj_String(name)
  guard _args[2].0 is YYObj_Expr(_dollar3)
  YYObj_Prop(
    {
      ()
      Labeled(name, _dollar3)
    },
  )
}

// file:///./parser.mbty
// 161|    qual=LIDENT name=DOT_LIDENT props=properties { Apply({qual: Some(qual), name}, props) }

///|
fn yy_action_5(
  _last_pos : Position,
  _args : ArrayView[(YYObj, Position, Position)],
) -> YYObj {
  guard _args[0].0 is YYObj_String(qual)
  guard _args[1].0 is YYObj_String(name)
  guard _args[2].0 is YYObj_List_Prop_(props)
  YYObj_Expr(
    {
      ()
      Apply({ qual: Some(qual), name }, props)
    },
  )
}

// file:///./parser.mbty
// 159|    qual=LIDENT name=DOT_LIDENT                  { Ident({qual: Some(qual), name}) }

///|
fn yy_action_6(
  _last_pos : Position,
  _args : ArrayView[(YYObj, Position, Position)],
) -> YYObj {
  guard _args[0].0 is YYObj_String(qual)
  guard _args[1].0 is YYObj_String(name)
  YYObj_Expr(
    {
      ()
      Ident({ qual: Some(qual), name })
    },
  )
}

// file:///./parser.mbty
// 160|    name=LIDENT props=properties                 { Apply({qual: None, name}, props) }

///|
fn yy_action_7(
  _last_pos : Position,
  _args : ArrayView[(YYObj, Position, Position)],
) -> YYObj {
  guard _args[0].0 is YYObj_String(name)
  guard _args[1].0 is YYObj_List_Prop_(props)
  YYObj_Expr(
    {
      ()
      Apply({ qual: None, name }, props)
    },
  )
}

// file:///./parser.mbty
// 158|    name=LIDENT                                  { Ident({qual: None, name}) }

///|
fn yy_action_8(
  _last_pos : Position,
  _args : ArrayView[(YYObj, Position, Position)],
) -> YYObj {
  guard _args[0].0 is YYObj_String(name)
  YYObj_Expr(
    {
      ()
      Ident({ qual: None, name })
    },
  )
}

// file:///./parser.mbty
// 155|    expr                   { Expr($1) }

///|
fn yy_action_9(
  _last_pos : Position,
  _args : ArrayView[(YYObj, Position, Position)],
) -> YYObj {
  guard _args[0].0 is YYObj_Expr(_dollar1)
  YYObj_Prop(
    {
      ()
      Expr(_dollar1)
    },
  )
}

// file:///./parser.mbty
// 162|    STRING                                       { String($1) }

///|
fn yy_action_10(
  _last_pos : Position,
  _args : ArrayView[(YYObj, Position, Position)],
) -> YYObj {
  guard _args[0].0 is YYObj_StringLiteral(_dollar1)
  YYObj_Expr(
    {
      ()
      String(_dollar1)
    },
  )
}

// file:///./parser.mbty
// 163|    TRUE                                         { Bool(true) }

///|
fn yy_action_11(
  _last_pos : Position,
  _args : ArrayView[(YYObj, Position, Position)],
) -> YYObj {
  YYObj_Expr(
    {
      ()
      Bool(true)
    },
  )
}

// file:///./parser.mbty
// 164|    FALSE                                        { Bool(false) }

///|
fn yy_action_12(
  _last_pos : Position,
  _args : ArrayView[(YYObj, Position, Position)],
) -> YYObj {
  YYObj_Expr(
    {
      ()
      Bool(false)
    },
  )
}

// file:///./parser.mbty
// 172|    x=property "," xs=non_empty_properties{ xs.add(x) }

///|
fn yy_action_13(
  _last_pos : Position,
  _args : ArrayView[(YYObj, Position, Position)],
) -> YYObj {
  guard _args[0].0 is YYObj_Prop(x)
  guard _args[2].0 is YYObj_List_Prop_(xs)
  YYObj_List_Prop_(
    {
      ()
      xs.add(x)
    },
  )
}

// file:///./parser.mbty
// 171|    property { @list.from_array([$1]) }

///|
fn yy_action_14(
  _last_pos : Position,
  _args : ArrayView[(YYObj, Position, Position)],
) -> YYObj {
  guard _args[0].0 is YYObj_Prop(_dollar1)
  YYObj_List_Prop_(
    {
      ()
      @list.from_array([_dollar1])
    },
  )
}

///|
fn yy_input(
  token : Token,
  _start_pos : Position,
  _end_pos : Position,
) -> (YYSymbol, YYObj) {
  match token {
    CHAR(data) => (T_CHAR, YYObj_CharLiteral(data))
    INT(data) => (T_INT, YYObj_String(data))
    BYTE(data) => (T_BYTE, YYObj_CharLiteral(data))
    BYTES(data) => (T_BYTES, YYObj_StringLiteral(data))
    FLOAT(data) => (T_FLOAT, YYObj_String(data))
    DOUBLE(data) => (T_DOUBLE, YYObj_String(data))
    STRING(data) => (T_STRING, YYObj_StringLiteral(data))
    MULTILINE_STRING(data) => (T_MULTILINE_STRING, YYObj_String(data))
    MULTILINE_INTERP(data) => (T_MULTILINE_INTERP, YYObj_InterpLiteral(data))
    INTERP(data) => (T_INTERP, YYObj_InterpLiteral(data))
    REGEX_LITERAL(data) => (T_REGEX_LITERAL, YYObj_String(data))
    REGEX_INTERP(data) => (T_REGEX_INTERP, YYObj_InterpLiteral(data))
    ATTRIBUTE(data) => (T_ATTRIBUTE, YYObj__String__String___String_(data))
    LIDENT(data) => (T_LIDENT, YYObj_String(data))
    UIDENT(data) => (T_UIDENT, YYObj_String(data))
    POST_LABEL(data) => (T_POST_LABEL, YYObj_String(data))
    COMMENT(data) => (T_COMMENT, YYObj_Comment(data))
    NEWLINE => (T_NEWLINE, YYObj_Void)
    INFIX1(data) => (T_INFIX1, YYObj_String(data))
    INFIX2(data) => (T_INFIX2, YYObj_String(data))
    INFIX3(data) => (T_INFIX3, YYObj_String(data))
    INFIX4(data) => (T_INFIX4, YYObj_String(data))
    AUGMENTED_ASSIGNMENT(data) => (T_AUGMENTED_ASSIGNMENT, YYObj_String(data))
    EOF => (T_EOF, YYObj_Void)
    FALSE => (T_FALSE, YYObj_Void)
    TRUE => (T_TRUE, YYObj_Void)
    PUB => (T_PUB, YYObj_Void)
    PRIV => (T_PRIV, YYObj_Void)
    READONLY => (T_READONLY, YYObj_Void)
    IMPORT => (T_IMPORT, YYObj_Void)
    EXTERN => (T_EXTERN, YYObj_Void)
    BREAK => (T_BREAK, YYObj_Void)
    CONTINUE => (T_CONTINUE, YYObj_Void)
    STRUCT => (T_STRUCT, YYObj_Void)
    ENUM => (T_ENUM, YYObj_Void)
    TRAIT => (T_TRAIT, YYObj_Void)
    DERIVE => (T_DERIVE, YYObj_Void)
    IMPL => (T_IMPL, YYObj_Void)
    WITH => (T_WITH, YYObj_Void)
    RAISE => (T_RAISE, YYObj_Void)
    THROW => (T_THROW, YYObj_Void)
    TRY => (T_TRY, YYObj_Void)
    CATCH => (T_CATCH, YYObj_Void)
    ASYNC => (T_ASYNC, YYObj_Void)
    TYPEALIAS => (T_TYPEALIAS, YYObj_Void)
    TRAITALIAS => (T_TRAITALIAS, YYObj_Void)
    FNALIAS => (T_FNALIAS, YYObj_Void)
    EQUAL => (T_EQUAL, YYObj_Void)
    LPAREN => (T_LPAREN, YYObj_Void)
    RPAREN => (T_RPAREN, YYObj_Void)
    COMMA => (T_COMMA, YYObj_Void)
    MINUS => (T_MINUS, YYObj_Void)
    QUESTION => (T_QUESTION, YYObj_Void)
    EXCLAMATION => (T_EXCLAMATION, YYObj_Void)
    DOT_LIDENT(data) => (T_DOT_LIDENT, YYObj_String(data))
    DOT_UIDENT(data) => (T_DOT_UIDENT, YYObj_String(data))
    DOT_INT(data) => (T_DOT_INT, YYObj_Int(data))
    DOT_LPAREN => (T_DOT_LPAREN, YYObj_Void)
    COLONCOLON => (T_COLONCOLON, YYObj_Void)
    COLON => (T_COLON, YYObj_Void)
    SEMI(data) => (T_SEMI, YYObj_Bool(data))
    LBRACKET => (T_LBRACKET, YYObj_Void)
    PLUS => (T_PLUS, YYObj_Void)
    RBRACKET => (T_RBRACKET, YYObj_Void)
    UNDERSCORE => (T_UNDERSCORE, YYObj_Void)
    BAR => (T_BAR, YYObj_Void)
    LBRACE => (T_LBRACE, YYObj_Void)
    RBRACE => (T_RBRACE, YYObj_Void)
    AMPERAMPER => (T_AMPERAMPER, YYObj_Void)
    AMPER => (T_AMPER, YYObj_Void)
    CARET => (T_CARET, YYObj_Void)
    BARBAR => (T_BARBAR, YYObj_Void)
    PACKAGE_NAME(data) => (T_PACKAGE_NAME, YYObj_String(data))
    AS => (T_AS, YYObj_Void)
    PIPE => (T_PIPE, YYObj_Void)
    ELSE => (T_ELSE, YYObj_Void)
    FN => (T_FN, YYObj_Void)
    IF => (T_IF, YYObj_Void)
    LET => (T_LET, YYObj_Void)
    CONST => (T_CONST, YYObj_Void)
    MATCH => (T_MATCH, YYObj_Void)
    USING => (T_USING, YYObj_Void)
    MUTABLE => (T_MUTABLE, YYObj_Void)
    TYPE => (T_TYPE, YYObj_Void)
    FAT_ARROW => (T_FAT_ARROW, YYObj_Void)
    THIN_ARROW => (T_THIN_ARROW, YYObj_Void)
    WHILE => (T_WHILE, YYObj_Void)
    RETURN => (T_RETURN, YYObj_Void)
    DOTDOT => (T_DOTDOT, YYObj_Void)
    RANGE_INCLUSIVE => (T_RANGE_INCLUSIVE, YYObj_Void)
    RANGE_LT_INCLUSIVE => (T_RANGE_LT_INCLUSIVE, YYObj_Void)
    RANGE_EXCLUSIVE => (T_RANGE_EXCLUSIVE, YYObj_Void)
    RANGE_INCLUSIVE_REV => (T_RANGE_INCLUSIVE_REV, YYObj_Void)
    RANGE_EXCLUSIVE_REV => (T_RANGE_EXCLUSIVE_REV, YYObj_Void)
    ELLIPSIS => (T_ELLIPSIS, YYObj_Void)
    TEST => (T_TEST, YYObj_Void)
    LOOP => (T_LOOP, YYObj_Void)
    GUARD => (T_GUARD, YYObj_Void)
    DEFER => (T_DEFER, YYObj_Void)
    FOR => (T_FOR, YYObj_Void)
    IN => (T_IN, YYObj_Void)
    IS => (T_IS, YYObj_Void)
    SUBERROR => (T_SUBERROR, YYObj_Void)
    AND => (T_AND, YYObj_Void)
    LETREC => (T_LETREC, YYObj_Void)
    ENUMVIEW => (T_ENUMVIEW, YYObj_Void)
    DECLARE => (T_DECLARE, YYObj_Void)
    NORAISE => (T_NORAISE, YYObj_Void)
    WHERE => (T_WHERE, YYObj_Void)
    TRY_QUESTION => (T_TRY_QUESTION, YYObj_Void)
    TRY_EXCLAMATION => (T_TRY_EXCLAMATION, YYObj_Void)
    LEXMATCH => (T_LEXMATCH, YYObj_Void)
    LEXMATCH_QUESTION => (T_LEXMATCH_QUESTION, YYObj_Void)
  }
}

// [0, payloads → • EOF, $]
// [1, payloads → • properties, $]
// [11, properties → • LPAREN RPAREN, $]
// [12, properties → • LPAREN non_empty_properties RPAREN, $]
// [15, payloads_prime → • payloads, $]

///|
fn yy_state_0(_lookahead : YYSymbol) -> YYDecision {
  match _lookahead {
    NT_payloads => Shift(yy_state_1)
    T_LPAREN => Shift(yy_state_2)
    NT_properties => Shift(yy_state_20)
    T_EOF => Shift(yy_state_21)
    _ => Error
  }
}

// [15, payloads_prime → payloads •, $]

///|
fn yy_state_1(_lookahead : YYSymbol) -> YYDecision {
  Accept
}

// [2, property → • LIDENT EQUAL expr, RPAREN / COMMA]
// [3, property → • expr, RPAREN / COMMA]
// [4, expr → • LIDENT, RPAREN / COMMA]
// [5, expr → • LIDENT DOT_LIDENT, RPAREN / COMMA]
// [6, expr → • LIDENT properties, RPAREN / COMMA]
// [7, expr → • LIDENT DOT_LIDENT properties, RPAREN / COMMA]
// [8, expr → • STRING, RPAREN / COMMA]
// [9, expr → • TRUE, RPAREN / COMMA]
// [10, expr → • FALSE, RPAREN / COMMA]
// [11, properties → LPAREN • RPAREN, $ / RPAREN / COMMA]
// [12, properties → LPAREN • non_empty_properties RPAREN, $ / RPAREN / COMMA]
// [13, non_empty_properties → • property, RPAREN]
// [14, non_empty_properties → • property COMMA non_empty_properties, RPAREN]

///|
fn yy_state_2(_lookahead : YYSymbol) -> YYDecision {
  match _lookahead {
    NT_property => Shift(yy_state_3)
    T_FALSE => Shift(yy_state_6)
    T_TRUE => Shift(yy_state_7)
    T_STRING => Shift(yy_state_8)
    NT_expr => Shift(yy_state_9)
    T_LIDENT => Shift(yy_state_10)
    NT_non_empty_properties => Shift(yy_state_17)
    T_RPAREN => Shift(yy_state_19)
    _ => Error
  }
}

// [13, non_empty_properties → property •, RPAREN]
// [14, non_empty_properties → property • COMMA non_empty_properties, RPAREN]

///|
fn yy_state_3(_lookahead : YYSymbol) -> YYDecision {
  match _lookahead {
    T_COMMA => Shift(yy_state_4)
    T_RPAREN => Reduce(1, NT_non_empty_properties, yy_action_14)
    _ => Error
  }
}

// [2, property → • LIDENT EQUAL expr, RPAREN / COMMA]
// [3, property → • expr, RPAREN / COMMA]
// [4, expr → • LIDENT, RPAREN / COMMA]
// [5, expr → • LIDENT DOT_LIDENT, RPAREN / COMMA]
// [6, expr → • LIDENT properties, RPAREN / COMMA]
// [7, expr → • LIDENT DOT_LIDENT properties, RPAREN / COMMA]
// [8, expr → • STRING, RPAREN / COMMA]
// [9, expr → • TRUE, RPAREN / COMMA]
// [10, expr → • FALSE, RPAREN / COMMA]
// [13, non_empty_properties → • property, RPAREN]
// [14, non_empty_properties → • property COMMA non_empty_properties, RPAREN]
// [14, non_empty_properties → property COMMA • non_empty_properties, RPAREN]

///|
fn yy_state_4(_lookahead : YYSymbol) -> YYDecision {
  match _lookahead {
    NT_property => Shift(yy_state_3)
    NT_non_empty_properties => Shift(yy_state_5)
    T_FALSE => Shift(yy_state_6)
    T_TRUE => Shift(yy_state_7)
    T_STRING => Shift(yy_state_8)
    NT_expr => Shift(yy_state_9)
    T_LIDENT => Shift(yy_state_10)
    _ => Error
  }
}

// [14, non_empty_properties → property COMMA non_empty_properties •, RPAREN]

///|
fn yy_state_5(_lookahead : YYSymbol) -> YYDecision {
  ReduceNoLookahead(3, NT_non_empty_properties, yy_action_13)
}

// [10, expr → FALSE •, RPAREN / COMMA]

///|
fn yy_state_6(_lookahead : YYSymbol) -> YYDecision {
  ReduceNoLookahead(1, NT_expr, yy_action_12)
}

// [9, expr → TRUE •, RPAREN / COMMA]

///|
fn yy_state_7(_lookahead : YYSymbol) -> YYDecision {
  ReduceNoLookahead(1, NT_expr, yy_action_11)
}

// [8, expr → STRING •, RPAREN / COMMA]

///|
fn yy_state_8(_lookahead : YYSymbol) -> YYDecision {
  ReduceNoLookahead(1, NT_expr, yy_action_10)
}

// [3, property → expr •, RPAREN / COMMA]

///|
fn yy_state_9(_lookahead : YYSymbol) -> YYDecision {
  ReduceNoLookahead(1, NT_property, yy_action_9)
}

// [2, property → LIDENT • EQUAL expr, RPAREN / COMMA]
// [4, expr → LIDENT •, RPAREN / COMMA]
// [5, expr → LIDENT • DOT_LIDENT, RPAREN / COMMA]
// [6, expr → LIDENT • properties, RPAREN / COMMA]
// [7, expr → LIDENT • DOT_LIDENT properties, RPAREN / COMMA]
// [11, properties → • LPAREN RPAREN, RPAREN / COMMA]
// [12, properties → • LPAREN non_empty_properties RPAREN, RPAREN / COMMA]

///|
fn yy_state_10(_lookahead : YYSymbol) -> YYDecision {
  match _lookahead {
    T_LPAREN => Shift(yy_state_2)
    NT_properties => Shift(yy_state_11)
    T_DOT_LIDENT => Shift(yy_state_12)
    T_EQUAL => Shift(yy_state_14)
    T_RPAREN | T_COMMA => Reduce(1, NT_expr, yy_action_8)
    _ => Error
  }
}

// [6, expr → LIDENT properties •, RPAREN / COMMA]

///|
fn yy_state_11(_lookahead : YYSymbol) -> YYDecision {
  ReduceNoLookahead(2, NT_expr, yy_action_7)
}

// [5, expr → LIDENT DOT_LIDENT •, RPAREN / COMMA]
// [7, expr → LIDENT DOT_LIDENT • properties, RPAREN / COMMA]
// [11, properties → • LPAREN RPAREN, RPAREN / COMMA]
// [12, properties → • LPAREN non_empty_properties RPAREN, RPAREN / COMMA]

///|
fn yy_state_12(_lookahead : YYSymbol) -> YYDecision {
  match _lookahead {
    T_LPAREN => Shift(yy_state_2)
    NT_properties => Shift(yy_state_13)
    T_RPAREN | T_COMMA => Reduce(2, NT_expr, yy_action_6)
    _ => Error
  }
}

// [7, expr → LIDENT DOT_LIDENT properties •, RPAREN / COMMA]

///|
fn yy_state_13(_lookahead : YYSymbol) -> YYDecision {
  ReduceNoLookahead(3, NT_expr, yy_action_5)
}

// [2, property → LIDENT EQUAL • expr, RPAREN / COMMA]
// [4, expr → • LIDENT, RPAREN / COMMA]
// [5, expr → • LIDENT DOT_LIDENT, RPAREN / COMMA]
// [6, expr → • LIDENT properties, RPAREN / COMMA]
// [7, expr → • LIDENT DOT_LIDENT properties, RPAREN / COMMA]
// [8, expr → • STRING, RPAREN / COMMA]
// [9, expr → • TRUE, RPAREN / COMMA]
// [10, expr → • FALSE, RPAREN / COMMA]

///|
fn yy_state_14(_lookahead : YYSymbol) -> YYDecision {
  match _lookahead {
    T_FALSE => Shift(yy_state_6)
    T_TRUE => Shift(yy_state_7)
    T_STRING => Shift(yy_state_8)
    T_LIDENT => Shift(yy_state_15)
    NT_expr => Shift(yy_state_16)
    _ => Error
  }
}

// [4, expr → LIDENT •, RPAREN / COMMA]
// [5, expr → LIDENT • DOT_LIDENT, RPAREN / COMMA]
// [6, expr → LIDENT • properties, RPAREN / COMMA]
// [7, expr → LIDENT • DOT_LIDENT properties, RPAREN / COMMA]
// [11, properties → • LPAREN RPAREN, RPAREN / COMMA]
// [12, properties → • LPAREN non_empty_properties RPAREN, RPAREN / COMMA]

///|
fn yy_state_15(_lookahead : YYSymbol) -> YYDecision {
  match _lookahead {
    T_LPAREN => Shift(yy_state_2)
    NT_properties => Shift(yy_state_11)
    T_DOT_LIDENT => Shift(yy_state_12)
    T_RPAREN | T_COMMA => Reduce(1, NT_expr, yy_action_8)
    _ => Error
  }
}

// [2, property → LIDENT EQUAL expr •, RPAREN / COMMA]

///|
fn yy_state_16(_lookahead : YYSymbol) -> YYDecision {
  ReduceNoLookahead(3, NT_property, yy_action_4)
}

// [12, properties → LPAREN non_empty_properties • RPAREN, $ / RPAREN / COMMA]

///|
fn yy_state_17(_lookahead : YYSymbol) -> YYDecision {
  match _lookahead {
    T_RPAREN => Shift(yy_state_18)
    _ => Error
  }
}

// [12, properties → LPAREN non_empty_properties RPAREN •, $ / RPAREN / COMMA]

///|
fn yy_state_18(_lookahead : YYSymbol) -> YYDecision {
  ReduceNoLookahead(3, NT_properties, yy_action_3)
}

// [11, properties → LPAREN RPAREN •, $ / RPAREN / COMMA]

///|
fn yy_state_19(_lookahead : YYSymbol) -> YYDecision {
  ReduceNoLookahead(2, NT_properties, yy_action_2)
}

// [1, payloads → properties •, $]

///|
fn yy_state_20(_lookahead : YYSymbol) -> YYDecision {
  ReduceNoLookahead(1, NT_payloads, yy_action_1)
}

// [0, payloads → EOF •, $]

///|
fn yy_state_21(_lookahead : YYSymbol) -> YYDecision {
  ReduceNoLookahead(1, NT_payloads, yy_action_0)
}

///|
fn[T] yy_parse(
  tokens : Array[(Token, Position, Position)],
  start : YYState,
  return_ : (YYObj) -> T,
  initial_pos? : Position,
) -> T raise ParseError {
  let mut cursor = 0
  let mut state_stack : @list.List[YYState] = @list.cons(start, @list.empty())
  let data_stack : Array[(YYObj, Position, Position)] = []
  let mut last_pos = initial_pos.unwrap_or(tokens[0].1)
  let mut state = start
  let mut lookahead : (YYSymbol, (YYObj, Position, Position), Token?)? = None
  let mut last_shifted_state_stack = state_stack
  while true {
    let decision = match state(EOI) {
      ReduceNoLookahead(_) | Accept as t => t
      _ =>
        match lookahead {
          Some(la) => state(la.0)
          None =>
            if cursor < tokens.length() {
              let (token, start_pos, end_pos) = tokens[cursor]
              cursor += 1
              let (symbol, data) = yy_input(token, start_pos, end_pos)
              lookahead = Some(
                (symbol, (data, start_pos, end_pos), Some(token)),
              )
              state(symbol)
            } else {
              lookahead = Some((EOI, (YYObj_Void, last_pos, last_pos), None))
              state(EOI)
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
      | ReduceNoLookahead(count, symbol, action) =>
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
              | ReduceNoLookahead(count, symbol, action) =>
                continue (count, symbol, action)
              _ => panic()
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

///|
fn error(
  stack : @list.List[YYState],
  token : Token?,
  loc : (Position, Position),
) -> Unit raise ParseError {
  let expected = []
  fn try_add(symbol : YYSymbol, kind : TokenKind) {
    fn go(stack : @list.List[YYState]) {
      match stack {
        Empty => ()
        More(state, ..) =>
          match state(symbol) {
            Accept | Shift(_) => expected.push(kind)
            Reduce(count, symbol, _) | ReduceNoLookahead(count, symbol, _) => {
              fn inner_go(stack : @list.List[YYState], count, symbol) {
                let stack = stack.drop(count)
                guard stack is More(state, ..)
                match state(symbol) {
                  Shift(state) => go(@list.cons(state, stack))
                  Reduce(count, symbol, _)
                  | ReduceNoLookahead(count, symbol, _) =>
                    inner_go(stack, count, symbol)
                  _ => panic()
                }
              }
              inner_go(stack, count, symbol)
            }
            Error => ()
          }
      }
    }
    go(stack)
  }
  for
    term in (
      [
        (T_CHAR, TK_CHAR),
        (T_INT, TK_INT),
        (T_BYTE, TK_BYTE),
        (T_BYTES, TK_BYTES),
        (T_FLOAT, TK_FLOAT),
        (T_DOUBLE, TK_DOUBLE),
        (T_STRING, TK_STRING),
        (T_MULTILINE_STRING, TK_MULTILINE_STRING),
        (T_MULTILINE_INTERP, TK_MULTILINE_INTERP),
        (T_INTERP, TK_INTERP),
        (T_REGEX_LITERAL, TK_REGEX_LITERAL),
        (T_REGEX_INTERP, TK_REGEX_INTERP),
        (T_ATTRIBUTE, TK_ATTRIBUTE),
        (T_LIDENT, TK_LIDENT),
        (T_UIDENT, TK_UIDENT),
        (T_POST_LABEL, TK_POST_LABEL),
        (T_COMMENT, TK_COMMENT),
        (T_NEWLINE, TK_NEWLINE),
        (T_INFIX1, TK_INFIX1),
        (T_INFIX2, TK_INFIX2),
        (T_INFIX3, TK_INFIX3),
        (T_INFIX4, TK_INFIX4),
        (T_AUGMENTED_ASSIGNMENT, TK_AUGMENTED_ASSIGNMENT),
        (T_EOF, TK_EOF),
        (T_FALSE, TK_FALSE),
        (T_TRUE, TK_TRUE),
        (T_PUB, TK_PUB),
        (T_PRIV, TK_PRIV),
        (T_READONLY, TK_READONLY),
        (T_IMPORT, TK_IMPORT),
        (T_EXTERN, TK_EXTERN),
        (T_BREAK, TK_BREAK),
        (T_CONTINUE, TK_CONTINUE),
        (T_STRUCT, TK_STRUCT),
        (T_ENUM, TK_ENUM),
        (T_TRAIT, TK_TRAIT),
        (T_DERIVE, TK_DERIVE),
        (T_IMPL, TK_IMPL),
        (T_WITH, TK_WITH),
        (T_RAISE, TK_RAISE),
        (T_THROW, TK_THROW),
        (T_TRY, TK_TRY),
        (T_CATCH, TK_CATCH),
        (T_ASYNC, TK_ASYNC),
        (T_TYPEALIAS, TK_TYPEALIAS),
        (T_TRAITALIAS, TK_TRAITALIAS),
        (T_FNALIAS, TK_FNALIAS),
        (T_EQUAL, TK_EQUAL),
        (T_LPAREN, TK_LPAREN),
        (T_RPAREN, TK_RPAREN),
        (T_COMMA, TK_COMMA),
        (T_MINUS, TK_MINUS),
        (T_QUESTION, TK_QUESTION),
        (T_EXCLAMATION, TK_EXCLAMATION),
        (T_DOT_LIDENT, TK_DOT_LIDENT),
        (T_DOT_UIDENT, TK_DOT_UIDENT),
        (T_DOT_INT, TK_DOT_INT),
        (T_DOT_LPAREN, TK_DOT_LPAREN),
        (T_COLONCOLON, TK_COLONCOLON),
        (T_COLON, TK_COLON),
        (T_SEMI, TK_SEMI),
        (T_LBRACKET, TK_LBRACKET),
        (T_PLUS, TK_PLUS),
        (T_RBRACKET, TK_RBRACKET),
        (T_UNDERSCORE, TK_UNDERSCORE),
        (T_BAR, TK_BAR),
        (T_LBRACE, TK_LBRACE),
        (T_RBRACE, TK_RBRACE),
        (T_AMPERAMPER, TK_AMPERAMPER),
        (T_AMPER, TK_AMPER),
        (T_CARET, TK_CARET),
        (T_BARBAR, TK_BARBAR),
        (T_PACKAGE_NAME, TK_PACKAGE_NAME),
        (T_AS, TK_AS),
        (T_PIPE, TK_PIPE),
        (T_ELSE, TK_ELSE),
        (T_FN, TK_FN),
        (T_IF, TK_IF),
        (T_LET, TK_LET),
        (T_CONST, TK_CONST),
        (T_MATCH, TK_MATCH),
        (T_USING, TK_USING),
        (T_MUTABLE, TK_MUTABLE),
        (T_TYPE, TK_TYPE),
        (T_FAT_ARROW, TK_FAT_ARROW),
        (T_THIN_ARROW, TK_THIN_ARROW),
        (T_WHILE, TK_WHILE),
        (T_RETURN, TK_RETURN),
        (T_DOTDOT, TK_DOTDOT),
        (T_RANGE_INCLUSIVE, TK_RANGE_INCLUSIVE),
        (T_RANGE_LT_INCLUSIVE, TK_RANGE_LT_INCLUSIVE),
        (T_RANGE_EXCLUSIVE, TK_RANGE_EXCLUSIVE),
        (T_RANGE_INCLUSIVE_REV, TK_RANGE_INCLUSIVE_REV),
        (T_RANGE_EXCLUSIVE_REV, TK_RANGE_EXCLUSIVE_REV),
        (T_ELLIPSIS, TK_ELLIPSIS),
        (T_TEST, TK_TEST),
        (T_LOOP, TK_LOOP),
        (T_GUARD, TK_GUARD),
        (T_DEFER, TK_DEFER),
        (T_FOR, TK_FOR),
        (T_IN, TK_IN),
        (T_IS, TK_IS),
        (T_SUBERROR, TK_SUBERROR),
        (T_AND, TK_AND),
        (T_LETREC, TK_LETREC),
        (T_ENUMVIEW, TK_ENUMVIEW),
        (T_DECLARE, TK_DECLARE),
        (T_NORAISE, TK_NORAISE),
        (T_WHERE, TK_WHERE),
        (T_TRY_QUESTION, TK_TRY_QUESTION),
        (T_TRY_EXCLAMATION, TK_TRY_EXCLAMATION),
        (T_LEXMATCH, TK_LEXMATCH),
        (T_LEXMATCH_QUESTION, TK_LEXMATCH_QUESTION),
      ] : Array[(YYSymbol, TokenKind)]) {
    try_add(term.0, term.1)
  }
  match token {
    None => raise UnexpectedEndOfInput(loc.1, expected)
    Some(token) => raise UnexpectedToken(token, loc, expected)
  }
}

///|
pub fn payloads(
  tokens : Array[(Token, Position, Position)],
  initial_pos? : Position,
) -> List[Prop] raise ParseError {
  yy_parse(
    tokens,
    yy_state_0,
    it => {
      guard it is YYObj_List_Prop_(result)
      result
    },
    initial_pos?,
  )
}
