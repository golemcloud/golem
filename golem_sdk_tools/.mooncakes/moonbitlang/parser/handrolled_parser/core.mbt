///|
priv enum Mode {
  Normal
  Panic(fail_at~ : TokenKind, sync_at~ : Int, loc~ : Location)
}

///|
let dummy_pos : Position = { fname: "", lnum: 0, bol: 0, cnum: 0 }

///|
/// suppress the `fail_at is unused` warning
test {
  match
    Panic(fail_at=TK_AMPER, sync_at=1, loc={ start: dummy_pos, end: dummy_pos }) {
    Normal => ()
    Panic(fail_at~, ..) => fail_at |> ignore
  }
}

///|
priv struct State {
  tokens : Array[Triple]
  diagnostics : Array[Report]
  mut next : Int
  syncs : Array[TokenKind]
  mut mode : Mode
  mut parsed_position : Position
}

// ///|
// impl Show for State with output(parser, buf) {
//   let { tokens: _, diagnostics: _, next: _, syncs, mode, parsed_position: _ } = parser
//   buf
//   ..write_string("{ ")
//   ..write_string("next: ")
//   ..write_string(parser.peek_token().to_string())
//   ..write_string("\n, syncs: ")
//   ..write_string(syncs.to_string())
//   ..write_string("\n, mode: ")
//   ..write_string(mode.to_string())
//   ..write_string("\n}")
// }

///|
fn inidex_of_token_kind(xs : Array[TokenKind], kind : TokenKind) -> Int? {
  let mut i = xs.length() - 1
  while i >= 0 {
    if xs[i] == kind {
      return Some(i)
    }
    i -= 1
  }
  None
}

///|
/// Peek the nth token in the state.
/// 
/// peek(nth=0) returns the next not consumed token
/// peek(nth=1) returns the next token after the next not consumed token
fn State::peek(state : Self, nth? : Int = 0) -> Triple {
  let mut ofs = 0
  let mut step = 0
  while state.tokens.get(state.next + ofs) is Some((token, _, _) as triple) {
    match token {
      NEWLINE | COMMENT(_) => ofs += 1
      EOF => return triple
      _ =>
        if step == nth {
          return triple
        } else {
          step += 1
          ofs += 1
          continue
        }
    }
  }
  guard state.tokens.last() is Some((_, last_spos, last_epos))
  return (EOF, last_spos, last_epos)
}

///|
fn State::consume(state : Self) -> Triple {
  while state.tokens.get(state.next) is Some((token, _, _) as triple) {
    match token {
      NEWLINE | COMMENT(_) => state.next += 1
      EOF => return triple
      _ => {
        // println("consumed: \{state.peek_token()}")
        state.next += 1
        state.parsed_position = state.previous_triple().2
        return triple
      }
    }
  }
  panic()
}

///|
fn State::peek_token(state : Self, nth? : Int = 0) -> Token {
  state.peek(nth~).0
}

///|
fn State::peek_kind(state : Self, nth? : Int = 0) -> TokenKind {
  state.peek(nth~).0.kind()
}

///|
fn State::peek_spos(state : Self, nth? : Int = 0) -> Position {
  state.peek(nth~).1
}

///|
fn State::peek_epos(state : Self, nth? : Int = 0) -> Position {
  state.peek(nth~).2
}

///|
fn State::loc_start_with(state : Self, start : Position) -> Location {
  { start, end: state.parsed_position }
}

///|
fn State::peek_location(state : Self) -> Location {
  let (_, start, end) = state.peek()
  { start, end }
}

///|
fn State::skip(state : Self) -> Unit {
  ignore(state.consume())
}

///|
fn State::previous_triple(state : Self) -> Triple {
  if state.next == 0 {
    state.tokens[0]
  } else {
    state.tokens[state.next - 1]
  }
}

///|
/// Enter the panic mode. 
/// This function will record the panic position and trying to find out 
/// a reocver position and sync token immediately.
fn State::panic(state : Self) -> Location {
  match state.mode {
    Normal => {
      let prev_end = state.previous_triple().2
      let (failed_at, first_start, _) = state.peek()
      let mut skipped = 0
      for {
        let last_end = state.previous_triple().2
        let (next_token, next_start, _) = state.peek()
        match inidex_of_token_kind(state.syncs, next_token.kind()) {
          Some(index) => {
            let loc : Location = if skipped > 0 {
              { start: first_start, end: last_end }
            } else {
              state.parsed_position = next_start
              { start: prev_end, end: next_start }
            }
            state.mode = Panic(fail_at=failed_at.kind(), sync_at=index, loc~)
            return loc
          }
          None => {
            state.skip()
            skipped += 1
          }
        }
      }
      panic()
    }
    Panic(loc~, ..) => loc
  }
}

///|
/// Set the state to normal mode.
fn State::recover(state : Self) -> Unit {
  match state.mode {
    Normal => abort("Cannot recover from normal mode")
    Panic(_, ..) => state.mode = Normal
  }
}

///|
fn State::report_failed_to_parse(
  state : Self,
  found : Token,
  expected : String,
  loc : Location,
) -> Unit {
  let msg = match found {
    SEMI(false) =>
      "Unexpected line break here, missing \{expected} at the end of this line."
    EOF => "Unexpected end of file, missing \{expected} here."
    _ =>
      "Unexpected token \{found.to_expect_string()}, you may expect \{expected}."
  }
  state.diagnostics.push({ loc, msg })
}

///|
fn expect_tokens_to_string(expect_tokens : Array[TokenKind]) -> String {
  match expect_tokens {
    [] => ""
    [x] => x.to_expect_string()
    xs => xs.iter().map(x => x.to_expect_string()).join(", ")
  }
}

///|
fn State::report_unexpected(
  state : Self,
  found : Token,
  expect : Array[TokenKind],
  loc : Location,
) -> Unit {
  state.report_failed_to_parse(found, expect_tokens_to_string(expect), loc)
}

///|
fn State::report_error(state : Self, report : Report) -> Unit {
  state.diagnostics.push(report)
}

///|
fn State::push_sync(state : Self, sync : TokenKind) -> Unit {
  state.syncs.push(sync)
}

///|
fn State::pop_sync(state : Self, sync : TokenKind) -> Unit {
  match state.syncs.pop() {
    Some(x) => {
      guard x == sync
      match state.mode {
        Panic(sync_at~, ..) if sync_at == state.syncs.length() =>
          state.recover()
        _ => ()
      }
    }
    None => abort("Cannot pop sync token, no sync token is available.")
  }
}

///|
fn State::push_syncs(state : Self, syncs : Array[TokenKind]) -> Unit {
  syncs.each(x => state.push_sync(x))
}

///|
fn State::pop_syncs(state : Self, syncs : Array[TokenKind]) -> Unit {
  syncs.rev_each(x => state.pop_sync(x))
}

///|
fn[A] State::with_syncs(
  state : Self,
  syncs : Array[TokenKind],
  f : (Self) -> A,
) -> A {
  state.push_syncs(syncs)
  let result = f(state)
  state.pop_syncs(syncs)
  result
}

///|
fn State::expect_token(state : Self, expected : TokenKind) -> Unit {
  match state.mode {
    Normal => {
      let (tok, start, end) = state.peek()
      if tok.kind() == expected {
        state.skip()
      } else {
        state.panic() |> ignore
        state.report_unexpected(tok, [expected], { start, end })
      }
    }
    Panic(_) => ()
  }
}

///|
fn State::expect_lident(state : Self, context~ : String) -> String {
  match state.mode {
    Panic(_) => ""
    Normal =>
      match state.peek_token() {
        LIDENT(name) => {
          state.skip()
          name
        }
        UIDENT(name) => {
          let loc = state.peek_location()
          state.skip()
          state.report_error({
            loc,
            msg: "Unexpected uppercase identifier \{name}, expected lowercase identifier in \{context}.",
          })
          ""
        }
        other => {
          let loc = state.peek_location()
          state.report_unexpected(other, [TK_LIDENT], loc)
          state.panic() |> ignore
          ""
        }
      }
  }
}

///|
fn State::expect_uident(state : Self, context~ : String) -> String {
  match state.mode {
    Panic(_) => ""
    Normal =>
      match state.peek_token() {
        UIDENT(name) => {
          state.skip()
          name
        }
        LIDENT(name) => {
          let loc = state.peek_location()
          state.skip()
          state.report_error({
            loc,
            msg: "Unexpected lowercase identifier \{name}, expected uppercase identifier in \{context}.",
          })
          ""
        }
        other => {
          let loc = state.peek_location()
          state.report_unexpected(other, [TK_LIDENT], loc)
          state.panic() |> ignore
          ""
        }
      }
  }
}

///|
fn[A] State::expect(
  self : State,
  expected : TokenKind,
  f : (Token) -> A?,
  default~ : A,
) -> A {
  match self.mode {
    Panic(_) => default
    Normal => {
      let (tok, start, end) = self.peek()
      match f(tok) {
        Some(result) => {
          self.skip()
          result
        }
        None => {
          self.panic() |> ignore
          self.report_unexpected(tok, [expected], { start, end })
          default
        }
      }
    }
  }
}

///|
fn State::expect_string(self : Self) -> String {
  self.expect(TK_STRING, default="", tok => {
    match tok {
      STRING(s) => Some(s)
      _ => None
    }
  })
}

///|
// fn State::expect_pkg_name(self : Self) -> String {
//   self.expect(TK_PACKAGE_NAME, default="", tok => match tok {
//     PACKAGE_NAME(s) => Some(s)
//     _ => None
//   })
// }

///|
fn State::expect_dot_lident(self : Self, context~ : String) -> String {
  match self.mode {
    Panic(_) => ""
    Normal =>
      match self.peek_token() {
        DOT_LIDENT(name) => {
          self.skip()
          name
        }
        DOT_UIDENT(name) => {
          let loc = self.peek_location()
          self.skip()
          self.report_error({
            loc,
            msg: "Unexpected uppercase identifier \{name}, expected uppercase identifier in \{context}.",
          })
          ""
        }
        other => {
          let loc = self.peek_location()
          self.report_unexpected(other, [TK_DOT_LIDENT], loc)
          self.panic() |> ignore
          ""
        }
      }
  }
}

///|
fn State::expect_dot_uident(self : Self, context~ : String) -> String {
  match self.mode {
    Panic(_) => ""
    Normal =>
      match self.peek_token() {
        DOT_UIDENT(name) => {
          self.skip()
          name
        }
        DOT_LIDENT(name) => {
          let loc = self.peek_location()
          self.skip()
          self.report_error({
            loc,
            msg: "Unexpected lowercase identifier \{name}, expected uppercase identifier in \{context}.",
          })
          ""
        }
        other => {
          let loc = self.peek_location()
          self.report_unexpected(other, [TK_DOT_UIDENT], loc)
          self.panic() |> ignore
          ""
        }
      }
  }
}

///|
fn State::expect_dot_int(self : Self) -> Int {
  self.expect(TK_PACKAGE_NAME, default=0, tok => {
    match tok {
      DOT_INT(s) => Some(s)
      _ => None
    }
  })
}

///|
fn State::expect_int(self : Self) -> String {
  self.expect(TK_INT, default="", tok => {
    match tok {
      INT(s) => Some(s)
      _ => None
    }
  })
}

///|
fn[A] State::optional(
  state : Self,
  first_set : Array[TokenKind],
  f : (State) -> A,
) -> A? {
  match state.mode {
    Normal if first_set.contains(state.peek_kind()) => Some(f(state))
    _ => None
  }
}

///|
/// Parse a list of elements separated by a delimiter, the list can be empty.
/// 
/// Note: The trailing delimiter is not allowed.
fn[A] State::sepby(
  self : Self,
  first_set~ : Array[TokenKind],
  delim~ : TokenKind,
  f : (State) -> A,
) -> Array[A] {
  match self.mode {
    Panic(_) => []
    Normal => {
      self.push_sync(delim)
      if first_set.contains(self.peek_kind()) {
        let result = [f(self)]
        while self.peek_kind() == delim {
          self..pop_sync(delim)..expect_token(delim).push_sync(delim)
          result.push(f(self))
        }
        self.pop_sync(delim)
        result
      } else {
        self.pop_sync(delim)
        []
      }
    }
  }
}

///|
/// Parse a list of elements separated by a delimiter, with at least one element.
/// 
/// Note: The trailing delimiter is not allowed.
fn[A] State::sepby1(
  self : Self,
  delim~ : TokenKind,
  f : (State) -> A,
) -> Array[A] {
  match self.mode {
    Panic(_) => []
    Normal => {
      self.push_sync(delim)
      let result = [f(self)]
      while self.peek_kind() == delim {
        self..pop_sync(delim)..expect_token(delim).push_sync(delim)
        result.push(f(self))
      }
      self.pop_sync(delim)
      result
    }
  }
}

///|
fn[A] State::surround(
  self : Self,
  left~ : TokenKind,
  right~ : TokenKind,
  f : (State) -> A,
) -> A {
  match self.mode {
    Panic(_) => f(self)
    Normal => {
      self..expect_token(left).push_sync(right)
      let result = f(self)
      self..pop_sync(right).expect_token(right)
      result
    }
  }
}

///|
fn[A] State::with_follow(
  self : Self,
  follow_set~ : Array[TokenKind],
  f : (Self) -> A,
) -> A {
  match self.mode {
    Panic(_) => f(self)
    Normal => {
      self.push_syncs(follow_set)
      let result = f(self)
      if !follow_set.contains(self.peek_kind()) {
        self.report_unexpected(
          self.peek_token(),
          follow_set,
          self.peek_location(),
        )
        self.panic() |> ignore
      }
      self.pop_syncs(follow_set)
      result
    }
  }
}

///|
/// Parse a sequence of elements separated by a delimiter, possibly ending 
/// with a trailing delimiter, and followed by a set of tokens (not consumed).
/// 
/// This version requires at least one element.
fn[A] State::series1_with_follow(
  self : Self,
  invalid_delims? : Array[TokenKind] = [],
  delim~ : TokenKind,
  follow_set~ : Array[TokenKind],
  f : (State) -> A,
) -> (Array[A], Bool) {
  guard self.mode is Normal else { ([], false) }
  fn expect_element() -> A {
    self.push_sync(delim)
    let elem = f(self)
    // if error occurs in f, pop_sync will recover to the next delimiter position
    self.pop_sync(delim)
    elem
  }

  self.push_syncs(follow_set)
  let acc = [expect_element()]
  for {
    match self.mode {
      Panic(_) => break
      Normal =>
        // end of the series
        if follow_set.contains(self.peek_kind()) {
          break
        } else if self.peek_kind() == delim {
          // consume the delimiter
          self.expect_token(delim)
          // if the next token is not in the follow set, consume an element
          if !follow_set.contains(self.peek_kind()) {
            acc.push(expect_element())
            continue
          } else {
            break
          }
        } else if invalid_delims.contains(self.peek_kind()) {
          // error recovery: consume the wrong delimiter and element
          self.report_error({
            loc: self.peek_location(),
            msg: "Expecting a newline or `;` here, but encountered \{delim.to_expect_string()}.",
          })
          self.skip()
          // if the next token is not in the follow set, consume an element
          if !follow_set.contains(self.peek_kind()) {
            acc.push(expect_element())
            continue
          } else {
            break
          }
        } else {
          // Error recovery: try to skip until the next delimiter.
          //
          // This branch is taken when the previous loop iteration already consumed
          // an element, and the next token is neither a delimiter nor in the follow set.
          // 
          // For example, in `(A, B C D, E)`, after consuming `A,` and `B`, we see `C` which is
          // neither a `,` nor a `)`, so we skip until the next `,` or `)` and continue parsing.
          let tok = self.peek_token()
          self.push_sync(delim)
          let loc = self.panic()
          self.report_unexpected(tok, [delim, ..follow_set], loc)
          self.pop_sync(delim)
          continue
        }
    }
  }
  self.pop_syncs(follow_set)
  let has_trailing_delim = self.previous_triple().0.kind() == delim
  (acc, has_trailing_delim)
}

///|
/// Parse a sequence of elements separated by a delimiter, possibly ending 
/// with a trailing delimiter, and followed by a set of tokens (not consumed).
fn[A] State::series_with_follow(
  self : Self,
  invalid_delims? : Array[TokenKind] = [],
  delim~ : TokenKind,
  follow_set? : Array[TokenKind] = [],
  f : (State) -> A,
) -> (Array[A], Bool) {
  fn try_recover_to_delim() {
    let loc = self.peek_location()
    let tok = self.peek_token()
    self.push_sync(delim)
    ignore(self.panic())
    self.report_unexpected(tok, [delim, ..follow_set], loc)
    self.pop_sync(delim)
    if self.peek_token().kind() == delim {
      self.expect_token(delim)
    }
  }

  match self.mode {
    Panic(_) => ([], false)
    Normal => {
      let result = []
      self.push_syncs(follow_set)
      for {
        match self.mode {
          Panic(_) => ()
          Normal if follow_set.contains(self.peek_kind()) => ()
          Normal => {
            self.push_sync(delim)
            result.push(f(self))
            self.pop_sync(delim)
            match self.mode {
              Panic(_) => ()
              Normal if follow_set.contains(self.peek_kind()) => ()
              Normal if self.peek_kind() == delim => {
                self.expect_token(delim)
                continue
              }
              Normal if invalid_delims.contains(self.peek_kind()) => {
                self
                ..report_error({
                  loc: self.peek_location(),
                  msg: "Expecting a newline or `;` here, but encountered \{delim.to_expect_string()}.",
                })
                .skip()
                continue
              }
              Normal => {
                try_recover_to_delim()
                continue
              }
            }
          }
        }
        break // TODO: report dead code after the dead loop
      }
      self.pop_syncs(follow_set)
      let has_trailing_delim = self.previous_triple().0.kind() == delim
      (result, has_trailing_delim)
    }
  }
}

///|
/// Parse a sequence of elements separated by a delimiter, possibly ending 
/// with a trailing delimiter, and surrounded by `left` and `right` tokens 
/// (which are not consumed).
fn[A] State::surround_series(
  self : Self,
  invalid_delims? : Array[TokenKind] = [],
  left~ : TokenKind,
  right~ : TokenKind,
  delim~ : TokenKind,
  f : (State) -> A,
) -> Array[A] {
  match self.mode {
    Panic(_) => []
    Normal =>
      self.surround(left~, right~, state => {
        state.series_with_follow(invalid_delims~, delim~, follow_set=[right], f).0
      }) // TODO(moonfmt): bad case
  }
}
