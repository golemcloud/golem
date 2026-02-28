///|
pub struct LexResult {
  tokens : Array[(Token, Position, Position)]
  errors : Array[(Position, Position, LexicalError)]
  docstrings : Array[@list.List[(Location, Comment)]]
}

///|
pub fn tokens_from_string(
  name? : String = "",
  start_pos? : Position = Position::{ fname: "", lnum: 1, bol: 0, cnum: 0 },
  is_interpolation? : Bool = false,
  comment~ : Bool,
  str : String,
) -> LexResult {
  let arr = Array::new(capacity=100)
  let (start_lnum, start_bol, start_cnum) = (
    start_pos.lnum,
    start_pos.bol,
    start_pos.cnum,
  )
  let env = LexEnv::{
    errors: [],
    docstrings: [],
    comment,
    file: name,
    tokens: arr,
    current_line: start_lnum,
    current_bol: start_bol,
    start_cnum,
    last_unhandled_comment: @ref.new(None),
    asi_context: ASIContext::new(),
    is_interpolation,
    surrogate_pair_count: 0,
  }
  lex_tokens(str, base=str, env~, preserve_comment=env.preserve_comment())
  let docstrings = env.docstrings
  if not(docstrings.is_empty()) {
    let last_idx = docstrings.length() - 1
    docstrings[last_idx] = docstrings[last_idx].rev()
  }
  docstrings.rev_in_place()
  LexResult::{ tokens: env.tokens, errors: env.errors, docstrings }
}

///|
pub fn tokens_from_string_with_utf16_location(
  name? : String = "",
  start_pos? : Position = Position::{ fname: "", lnum: 1, bol: 0, cnum: 0 },
  is_interpolation? : Bool = false,
  comment~ : Bool,
  str : String,
) -> LexResult {
  use_utf16_location.val = true
  let result = tokens_from_string(
    name~,
    start_pos~,
    is_interpolation~,
    comment~,
    str,
  )
  use_utf16_location.val = false
  return result
}
