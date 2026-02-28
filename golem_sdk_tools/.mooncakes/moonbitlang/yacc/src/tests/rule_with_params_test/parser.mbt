
pub(all) enum Token {
  LETTER(Char)
  LBRACE
  RBRACE
}

pub fn Token::kind(self : Token) -> TokenKind {
  match self {
    LETTER(_) => TK_LETTER
    LBRACE => TK_LBRACE
    RBRACE => TK_RBRACE
  }
}

pub(all) enum TokenKind {
  TK_LETTER
  TK_LBRACE
  TK_RBRACE
}

pub impl Show for TokenKind with output(self, logger) {
  logger.write_string(
    match self {
      TK_LETTER => "LETTER"
      TK_LBRACE => "\"{\""
      TK_RBRACE => "\"}\""
    }
  )
}

pub suberror ParseError {
  UnexpectedToken(Token, (Unit, Unit), Array[TokenKind])
  UnexpectedEndOfInput(Unit, Array[TokenKind])
}

type YYObj = Error

priv suberror YYObj_Void

priv suberror YYObj_Char {
  YYObj_Char(Char)
}

priv suberror YYObj_String {
  YYObj_String(String)
}

priv suberror YYObj__list_List_Char_ {
  YYObj__list_List_Char_(@list.List[Char])
}

type YYState = (YYSymbol) -> YYDecision

type YYAction = (Unit, ArrayView[(YYObj, Unit, Unit)]) -> YYObj

priv enum YYDecision {
  Accept
  Shift(YYState)
  Reduce(Int, YYSymbol, YYAction)
  ReduceNoLookahead(Int, YYSymbol, YYAction)
  Error
}

priv enum YYSymbol {
  T_LETTER
  T_LBRACE
  T_RBRACE
  NT_start
  NT_list_LETTER_
  NT_loption_delimited_LBRACE_list_LETTER__RBRACE__
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
// 9|    loption(delimited("{", list(LETTER), "}")) { $1.iter().map(fn (it) { String::make(1, it) }).join("") }
fn yy_action_0(_last_pos : Unit, _args : ArrayView[(YYObj, Unit, Unit)]) -> YYObj {
  guard _args[0].0 is YYObj__list_List_Char_(_dollar1)
  YYObj_String({();  _dollar1.iter().map(fn (it) { String::make(1, it) }).join("") })
}

// file:///./stdlib.mbty
// 45|    X list(X) { $2.add($1) }
fn yy_action_1(_last_pos : Unit, _args : ArrayView[(YYObj, Unit, Unit)]) -> YYObj {
  guard _args[1].0 is YYObj__list_List_Char_(_dollar2)
  guard _args[0].0 is YYObj_Char(_dollar1)
  YYObj__list_List_Char_({();  _dollar2.add(_dollar1) })
}

// file:///./stdlib.mbty
// 15|    X { $1 }
fn yy_action_2(_last_pos : Unit, _args : ArrayView[(YYObj, Unit, Unit)]) -> YYObj {
  let _sub_action_0_result = yy_action_3(_last_pos, _args[0:3])
  guard _sub_action_0_result is YYObj__list_List_Char_(_dollar1)
  YYObj__list_List_Char_({();  _dollar1 })
}

// file:///./parser.mbty
// 13|    OPEN X CLOSE { $2 }
fn yy_action_3(_last_pos : Unit, _args : ArrayView[(YYObj, Unit, Unit)]) -> YYObj {
  guard _args[1].0 is YYObj__list_List_Char_(_dollar2)
  YYObj__list_List_Char_({();  _dollar2 })
}

// file:///./stdlib.mbty
// 44|    { @list.empty() }
fn yy_action_4(_last_pos : Unit, _args : ArrayView[(YYObj, Unit, Unit)]) -> YYObj {
  YYObj__list_List_Char_({();  @list.empty() })
}

// file:///./stdlib.mbty
// 14|    { @list.empty() }
fn yy_action_5(_last_pos : Unit, _args : ArrayView[(YYObj, Unit, Unit)]) -> YYObj {
  YYObj__list_List_Char_({();  @list.empty() })
}

fn yy_input(token : Token, _start_pos : Unit, _end_pos : Unit) -> (YYSymbol, YYObj) {
  match token {
    LETTER(data) => (T_LETTER, YYObj_Char(data))
    LBRACE => (T_LBRACE, YYObj_Void)
    RBRACE => (T_RBRACE, YYObj_Void)
  }
}

// [0, start → • loption(delimited(LBRACE,list(LETTER),RBRACE)), $]
// [3, loption(delimited(LBRACE,list(LETTER),RBRACE)) → •, $]
// [4, loption(delimited(LBRACE,list(LETTER),RBRACE)) → • LBRACE list(LETTER) RBRACE, $]
// [5, start_prime → • start, $]
fn yy_state_0(_lookahead : YYSymbol) -> YYDecision {
  match _lookahead {
    NT_start => Shift(yy_state_1)
    T_LBRACE => Shift(yy_state_2)
    NT_loption_delimited_LBRACE_list_LETTER__RBRACE__ => Shift(yy_state_7)
    EOI => Reduce(0, NT_loption_delimited_LBRACE_list_LETTER__RBRACE__, yy_action_5)
    _ => Error
  }
}

// [5, start_prime → start •, $]
fn yy_state_1(_lookahead : YYSymbol) -> YYDecision {
  Accept
}

// [1, list(LETTER) → •, RBRACE]
// [2, list(LETTER) → • LETTER list(LETTER), RBRACE]
// [4, loption(delimited(LBRACE,list(LETTER),RBRACE)) → LBRACE • list(LETTER) RBRACE, $]
fn yy_state_2(_lookahead : YYSymbol) -> YYDecision {
  match _lookahead {
    NT_list_LETTER_ => Shift(yy_state_3)
    T_LETTER => Shift(yy_state_5)
    T_RBRACE => Reduce(0, NT_list_LETTER_, yy_action_4)
    _ => Error
  }
}

// [4, loption(delimited(LBRACE,list(LETTER),RBRACE)) → LBRACE list(LETTER) • RBRACE, $]
fn yy_state_3(_lookahead : YYSymbol) -> YYDecision {
  match _lookahead {
    T_RBRACE => Shift(yy_state_4)
    _ => Error
  }
}

// [4, loption(delimited(LBRACE,list(LETTER),RBRACE)) → LBRACE list(LETTER) RBRACE •, $]
fn yy_state_4(_lookahead : YYSymbol) -> YYDecision {
  ReduceNoLookahead(3, NT_loption_delimited_LBRACE_list_LETTER__RBRACE__, yy_action_2)
}

// [1, list(LETTER) → •, RBRACE]
// [2, list(LETTER) → • LETTER list(LETTER), RBRACE]
// [2, list(LETTER) → LETTER • list(LETTER), RBRACE]
fn yy_state_5(_lookahead : YYSymbol) -> YYDecision {
  match _lookahead {
    T_LETTER => Shift(yy_state_5)
    NT_list_LETTER_ => Shift(yy_state_6)
    T_RBRACE => Reduce(0, NT_list_LETTER_, yy_action_4)
    _ => Error
  }
}

// [2, list(LETTER) → LETTER list(LETTER) •, RBRACE]
fn yy_state_6(_lookahead : YYSymbol) -> YYDecision {
  ReduceNoLookahead(2, NT_list_LETTER_, yy_action_1)
}

// [0, start → loption(delimited(LBRACE,list(LETTER),RBRACE)) •, $]
fn yy_state_7(_lookahead : YYSymbol) -> YYDecision {
  ReduceNoLookahead(1, NT_start, yy_action_0)
}

fn[T] yy_parse(
  tokens : Array[(Token, Unit, Unit)],
  start : YYState,
  return_ : (YYObj) -> T,
  initial_pos? : Unit,
) -> T raise ParseError {
  let mut cursor = 0
  let mut state_stack : @list.List[YYState] = @list.cons(start, @list.empty())
  let data_stack : Array[(YYObj, Unit, Unit)] = []
  let mut last_pos = initial_pos.unwrap_or(tokens[0].1)
  let mut state = start
  let mut lookahead : Option[(YYSymbol, (YYObj, Unit, Unit), Token?)] = None
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

fn error(stack : @list.List[YYState], token : Token?, loc : (Unit, Unit)) -> Unit raise ParseError {
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
  for term in ([(T_LETTER, TK_LETTER), (T_LBRACE, TK_LBRACE), (T_RBRACE, TK_RBRACE)] : Array[(YYSymbol, TokenKind)]) {
    try_add(term.0, term.1)
  }
  match token {
    None => raise UnexpectedEndOfInput(loc.1, expected)
    Some(token) => raise UnexpectedToken(token, loc, expected)
  }
}

pub fn start(tokens : Array[(Token, Unit, Unit)], initial_pos? : Unit) -> String raise ParseError {
  yy_parse(
    tokens,
    yy_state_0,
    (it) => {
      guard it is YYObj_String(result)
      result
    },
    initial_pos?,
  )
}
