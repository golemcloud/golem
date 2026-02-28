///|
pub suberror LexError {
  Unrecognized(Char, Int)
} derive(Show)

///|
fn token(lexbuf : @lexbuf.StringLexbuf) -> (Token, Int, Int) raise LexError {
  // The matched pattern id
  let mut _match_pattern = @int.max_value
  let mut _match_start = lexbuf.curr_pos()
  let mut _match_end = -1
  let mut _capture_0_start = -1
  let mut _capture_0_end = -1
  let mut _capture_1_start = -1
  let mut _capture_1_end = -1
  let mut _capture_2_start = -1
  let mut _capture_2_end = -1
  let mut _capture_3_start = -1
  let mut _capture_3_end = -1
  let mut _tag_0 = -1
  let mut _tag_1 = -1
  loop 0 {
    0 =>
      continue match lexbuf.next_as_int() {
          -1 => 1
          0..=8 => 2
          9..=10 => 3
          11..=31 => 2
          32 => 3
          33..=39 => 2
          40 => 4
          41 => 5
          42..=43 => 2
          44 => 6
          45 => 7
          46..=62 => 2
          63 => 8
          64 => 9
          65..=90 => 10
          91 => 11
          92 => 2
          93 => 12
          94 => 2
          95 => 10
          96 => 2
          97..=122 => 10
          123..=1114111 => 2
          _ => break
        }
    1 => {
      _match_pattern = 11
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_end
      break
    }
    2 => {
      _match_pattern = 10
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_start + 1
      break
    }
    3 => {
      _match_pattern = 0
      _match_end = lexbuf.curr_pos()
      continue match lexbuf.next_as_int() {
          9..=10 => 3
          32 => 3
          _ => break
        }
    }
    4 => {
      _match_pattern = 6
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_start + 1
      break
    }
    5 => {
      _match_pattern = 7
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_start + 1
      break
    }
    6 => {
      _match_pattern = 8
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_start + 1
      break
    }
    7 => {
      _match_pattern = 10
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_start + 1
      continue match lexbuf.next_as_int() {
          62 => 13
          _ => break
        }
    }
    8 => {
      _match_pattern = 9
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_start + 1
      break
    }
    9 => {
      _match_pattern = 10
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_start + 1
      continue match lexbuf.next_as_int() {
          65..=90 => 14
          95 => 14
          97..=122 => 14
          _ => break
        }
    }
    10 => {
      _match_pattern = 2
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_end
      continue match lexbuf.next_as_int() {
          48..=57 => 10
          65..=90 => 10
          95 => 10
          97..=122 => 10
          _ => break
        }
    }
    11 => {
      _match_pattern = 4
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_start + 1
      break
    }
    12 => {
      _match_pattern = 5
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_start + 1
      break
    }
    13 => {
      _match_pattern = 3
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_start + 2
      break
    }
    14 => {
      _tag_0 = lexbuf.curr_pos()
      continue match lexbuf.next_as_int() {
          46 => 15
          47 => 17
          48..=57 => 14
          65..=90 => 14
          95 => 14
          97..=122 => 14
          _ => break
        }
    }
    15 => {
      _tag_1 = lexbuf.curr_pos()
      continue match lexbuf.next_as_int() {
          65..=90 => 16
          95 => 16
          97..=122 => 16
          _ => break
        }
    }
    16 => {
      _match_pattern = 1
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_end
      _capture_1_start = _tag_1
      _capture_1_end = _match_end
      _capture_2_start = _match_start + 1
      _capture_2_end = _tag_0
      continue match lexbuf.next_as_int() {
          48..=57 => 16
          65..=90 => 16
          95 => 16
          97..=122 => 16
          _ => break
        }
    }
    17 =>
      continue match lexbuf.next_as_int() {
          65..=90 => 14
          95 => 14
          97..=122 => 14
          _ => break
        }
    _ => panic()
  }
  guard _match_pattern <= 11 else {
    // No pattern matched
    panic()
  }
  lexbuf.reset(pos=_match_end)
  match _match_pattern {
    0 => {
      ()
      token(lexbuf)
    }
    1 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      let id = lexbuf.get_string(_capture_1_start, _capture_1_end)
      let pkg = lexbuf.get_string(_capture_2_start, _capture_2_end)
      (PKG_AND_IDENT((pkg, id)), _capture_0_start, _capture_0_end)
    }
    2 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      (IDENT(t), _capture_0_start, _capture_0_end)
    }
    3 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      (ARROW, _capture_0_start, _capture_0_end)
    }
    4 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      (LBRACKET, _capture_0_start, _capture_0_end)
    }
    5 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      (RBRACKET, _capture_0_start, _capture_0_end)
    }
    6 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      (LPAREN, _capture_0_start, _capture_0_end)
    }
    7 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      (RPAREN, _capture_0_start, _capture_0_end)
    }
    8 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      (COMMA, _capture_0_start, _capture_0_end)
    }
    9 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      (QUESTION, _capture_0_start, _capture_0_end)
    }
    10 => {
      ()
      let t = lexbuf.get_char(_capture_0_start, _capture_0_end)
      let _ = t
      raise Unrecognized(t, _capture_0_start)
    }
    11 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      (EOF, _capture_0_start, _capture_0_end)
    }
    _ => panic()
  }
}
