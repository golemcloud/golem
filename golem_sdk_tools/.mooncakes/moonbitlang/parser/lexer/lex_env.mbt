///|
priv struct LexEnv {
  errors : Array[(Position, Position, LexicalError)]
  docstrings : Array[@list.List[(Location, Comment)]]
  comment : Bool
  file : String
  tokens : Array[(Token, Position, Position)]
  mut current_line : Int
  mut current_bol : Int
  last_unhandled_comment : Ref[(Comment, Int)?]
  asi_context : ASIContext
  start_cnum : Int
  is_interpolation : Bool
  mut surrogate_pair_count : Int
}

///|
fn LexEnv::add_lexing_error(
  self : LexEnv,
  start~ : Int,
  end~ : Int,
  err : LexicalError,
) -> Unit {
  let loc_start = self.make_pos(start)
  let loc_end = self.make_pos(end)
  self.errors.push((loc_start, loc_end, err))
}

///|
fn LexEnv::make_pos(self : LexEnv, cnum : Int) -> Position {
  Position::{
    fname: self.file,
    lnum: self.current_line,
    cnum: self.start_cnum + cnum,
    bol: self.current_bol,
  }
}

///|
fn LexEnv::add_token(
  self : LexEnv,
  tok : Token,
  start : Position,
  end : Position,
) -> Unit {
  self.asi_context.add_token(
    tokens=self.tokens,
    last_unhandled_comment=self.last_unhandled_comment,
    tok,
  )
  // handle comment: add comments to token array
  self.tokens.push((tok, start, end))
  if self.comment {
    let tok_index = self.tokens.length() - 1
    fn prehandle_comment(c : Comment) -> Unit {
      self.last_unhandled_comment.val = Some((c, tok_index))
    }

    fn handle_comment(ci : (Comment, Int)) -> Unit {
      let (c, i) = ci
      let mut at_file_start = false
      fn count_newlines(start_index : Int, direction : Int) -> Int {
        let mut count = 0
        let mut index = start_index
        while true {
          let target_index = index + direction
          match self.tokens.get(target_index) {
            Some((COMMENT(_), _, _)) => return count
            Some((NEWLINE, _, _)) =>
              if count >= 2 {
                return count
              } else {
                count += 1
                index = target_index
              }
            None | Some((EOF, _, _)) => {
              if direction < 0 {
                at_file_start = true
              }
              return 0
            }
            Some(_) => return count
          }
        } nobreak {
          panic() // impossible
        }
      }

      let leading_newlines = @cmp.minimum(2, count_newlines(i, -1))
      // Make sure add trailing newline if the comment is at end of file
      let trailing_newlines = @cmp.minimum(2, count_newlines(i, 1))
      let (_, start, end) = self.tokens[i]
      let kind = if leading_newlines == 0 && not(at_file_start) {
        CommentKind::InlineTrailing
      } else {
        Ownline(
          leading_blank_line=leading_newlines == 2 && not(at_file_start),
          trailing_blank_line=trailing_newlines == 2,
        )
      }
      self.tokens[i] = (COMMENT({ ..c, kind, }), start, end)
    }

    match (self.last_unhandled_comment.val, tok) {
      (_, NEWLINE) => ()
      (Some(ci), COMMENT(c)) => {
        handle_comment(ci)
        prehandle_comment(c)
      }
      (None, COMMENT(c)) => prehandle_comment(c)
      (Some(ci), _) => handle_comment(ci)
      (None, _) => ()
    }
  }
}

///|
fn LexEnv::add_token_with_loc(
  self : LexEnv,
  tok : Token,
  start~ : Int,
  end~ : Int,
  start_offset? : Int = 0,
) -> Unit {
  let start = self.make_pos(start + start_offset)
  let end = self.make_pos(end)
  self.add_token(tok, start, end)
}

///|
fn LexEnv::preserve_comment(self : LexEnv) -> (Comment, Int, Int) -> Unit {
  if self.comment {
    fn(comment, start, end) {
      let docstrings = self.docstrings
      if comment.content is [_, _, '/', ..] {
        let loc = Location::{
          start: self.make_pos(start),
          end: self.make_pos(end),
        }
        if docstrings.is_empty() {
          docstrings.push(@list.singleton((loc, comment)))
        } else {
          guard docstrings.last() is Some(More(head, ..) as last)
          let last_idx = docstrings.length() - 1
          if loc.start.lnum - head.0.start.lnum > 1 {
            docstrings[last_idx] = last.rev()
            docstrings.push(@list.singleton((loc, comment)))
          } else {
            docstrings[last_idx] = docstrings[last_idx].add((loc, comment))
          }
        }
      }
    }
  } else {
    fn(_cmt, _start, _end) {  }
  }
}

///|
fn LexEnv::register_surrogate_pair(self : LexEnv, view : StringView) -> Unit {
  if !use_utf16_location.val {
    let mut count = 0
    for i = 0; i < view.length(); i = i + 1 {
      let c1 = view[i]
      if c1.is_leading_surrogate() && i + 1 < view.length() {
        let c2 = view[i + 1]
        if c2.is_trailing_surrogate() {
          count += 1
          continue i + 2
        }
      }
    }
    self.surrogate_pair_count += count
  }
}

///|
fn LexEnv::register_surrogate_pair_for_char(self : LexEnv, ch : Char) -> Unit {
  if !use_utf16_location.val {
    if ch.to_int() >= 0x10000 {
      self.surrogate_pair_count += 1
    }
  }
}

///|
fn LexEnv::calc_offset(
  self : LexEnv,
  base~ : StringView,
  view : StringView,
) -> Int {
  if use_utf16_location.val {
    view.start_offset() - base.start_offset()
  } else {
    view.start_offset() - base.start_offset() - self.surrogate_pair_count
  }
}
