///|
pub(all) struct Lexer(@lexbuf.StringLexbuf)

///|
let all_keywords : Map[String, Token] = {
  "and": AND,
  "as": AS,
  "assert": ASSERT,
  "begin": BEGIN,
  "class": CLASS,
  "constraint": CONSTRAINT,
  "do": DO,
  "done": DONE,
  "downto": DOWNTO,
  "else": ELSE,
  "end": END,
  "exception": EXCEPTION,
  "external": EXTERNAL,
  "false": FALSE,
  "for": FOR,
  "fun": FUN,
  "function": FUNCTION,
  "functor": FUNCTOR,
  "if": IF,
  "in": IN,
  "include": INCLUDE,
  "inherit": INHERIT,
  "initializer": INITIALIZER,
  "lazy": LAZY,
  "let": LET,
  "match": MATCH,
  "method": METHOD,
  "module": MODULE,
  "mutable": MUTABLE,
  "new": NEW,
  "nonrec": NONREC,
  "object": OBJECT,
  "of": OF,
  "open": OPEN,
  "or": OR,
  "private": PRIVATE,
  "rec": REC,
  "sig": SIG,
  "struct": STRUCT,
  "then": THEN,
  "do": TO,
  "true": TRUE,
  "try": TRY,
  "type": TYPE,
  "val": VAL,
  "virtual": VIRTUAL,
  "when": WHEN,
  "while": WHILE,
  "with": WITH,
}

///|
fn token(lexbuf : @lexbuf.StringLexbuf) -> (Token, Int, Int) {
  // The matched pattern id
  let mut _match_pattern = @int.max_value
  let mut _match_start = lexbuf.curr_pos()
  let mut _match_end = -1
  let mut _capture_0_start = -1
  let mut _capture_0_end = -1
  let mut _capture_1_start = -1
  let mut _capture_1_end = -1
  loop 0 {
    0 =>
      continue match lexbuf.next_as_int() {
          -1 => 1
          9..=10 => 2
          12..=13 => 2
          32 => 2
          33 => 3
          34 => 4
          35 => 5
          36 => 6
          37 => 7
          38 => 8
          40 => 9
          41 => 10
          42 => 7
          43 => 11
          44 => 12
          45 => 11
          46 => 13
          47 => 7
          48..=57 => 14
          58 => 15
          59 => 16
          60 => 17
          61 => 18
          62 => 19
          63 => 20
          64 => 21
          65..=90 => 22
          91 => 23
          93 => 24
          94 => 21
          95 => 25
          96 => 26
          97..=122 => 27
          123 => 28
          124 => 29
          125 => 30
          126 => 31
          _ => break
        }
    1 => {
      _match_pattern = 16
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_end
      break
    }
    2 => {
      _match_pattern = 0
      _match_end = lexbuf.curr_pos()
      continue match lexbuf.next_as_int() {
          9..=10 => 2
          12..=13 => 2
          32 => 2
          _ => break
        }
    }
    3 => {
      _match_pattern = 4
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_start + 1
      break
    }
    4 => {
      _match_pattern = 55
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_start + 1
      break
    }
    5 => {
      _match_pattern = 61
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_start + 1
      break
    }
    6 => {
      _match_pattern = 22
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_end
      continue match lexbuf.next_as_int() {
          33 => 6
          36..=38 => 6
          42..=43 => 6
          45..=47 => 6
          58 => 6
          60..=64 => 6
          94 => 6
          124 => 6
          126 => 6
          _ => break
        }
    }
    7 => {
      _match_pattern = 25
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_end
      continue match lexbuf.next_as_int() {
          33 => 7
          36..=38 => 7
          42..=43 => 7
          45..=47 => 7
          58 => 7
          60..=64 => 7
          94 => 7
          124 => 7
          126 => 7
          _ => break
        }
    }
    8 => {
      _match_pattern = 2
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_start + 1
      continue match lexbuf.next_as_int() {
          33 => 6
          36..=37 => 6
          38 => 48
          42..=43 => 6
          45..=47 => 6
          58 => 6
          60..=64 => 6
          94 => 6
          124 => 6
          126 => 6
          _ => break
        }
    }
    9 => {
      _match_pattern = 41
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_start + 1
      break
    }
    10 => {
      _match_pattern = 58
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_start + 1
      break
    }
    11 => {
      _match_pattern = 24
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_end
      continue match lexbuf.next_as_int() {
          33 => 11
          36..=38 => 11
          42..=43 => 11
          45..=47 => 11
          58 => 11
          60..=64 => 11
          94 => 11
          124 => 11
          126 => 11
          _ => break
        }
    }
    12 => {
      _match_pattern = 13
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_start + 1
      break
    }
    13 => {
      _match_pattern = 14
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_start + 1
      continue match lexbuf.next_as_int() {
          46 => 41
          _ => break
        }
    }
    14 => {
      _match_pattern = 28
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_end
      continue match lexbuf.next_as_int() {
          48..=57 => 14
          95 => 14
          _ => break
        }
    }
    15 => {
      _match_pattern = 9
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_start + 1
      continue match lexbuf.next_as_int() {
          58 => 42
          61 => 43
          62 => 44
          _ => break
        }
    }
    16 => {
      _match_pattern = 59
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_start + 1
      continue match lexbuf.next_as_int() {
          59 => 32
          _ => break
        }
    }
    17 => {
      _match_pattern = 22
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_end
      continue match lexbuf.next_as_int() {
          33 => 6
          36..=38 => 6
          42..=43 => 6
          45..=47 => 6
          58 => 6
          60..=64 => 6
          94 => 6
          116 => 45
          124 => 6
          126 => 6
          _ => break
        }
    }
    18 => {
      _match_pattern = 17
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_start + 1
      continue match lexbuf.next_as_int() {
          33 => 6
          36..=38 => 6
          42..=43 => 6
          45..=47 => 6
          58 => 6
          60..=64 => 6
          94 => 6
          124 => 6
          126 => 6
          _ => break
        }
    }
    19 => {
      _match_pattern = 19
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_start + 1
      continue match lexbuf.next_as_int() {
          33 => 6
          36..=38 => 6
          42..=43 => 6
          45..=47 => 6
          58 => 6
          60..=64 => 6
          93 => 39
          94 => 6
          124 => 6
          125 => 40
          126 => 6
          _ => break
        }
    }
    20 => {
      _match_pattern = 54
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_start + 1
      break
    }
    21 => {
      _match_pattern = 23
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_end
      continue match lexbuf.next_as_int() {
          33 => 21
          36..=38 => 21
          42..=43 => 21
          45..=47 => 21
          58 => 21
          60..=64 => 21
          94 => 21
          124 => 21
          126 => 21
          _ => break
        }
    }
    22 => {
      _match_pattern = 66
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_end
      continue match lexbuf.next_as_int() {
          48..=57 => 22
          65..=90 => 22
          95 => 22
          97..=122 => 22
          _ => break
        }
    }
    23 => {
      _match_pattern = 32
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_start + 1
      continue match lexbuf.next_as_int() {
          37 => 33
          60 => 34
          62 => 35
          64 => 36
          124 => 37
          _ => break
        }
    }
    24 => {
      _match_pattern = 57
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_start + 1
      break
    }
    25 => {
      _match_pattern = 67
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_start + 1
      break
    }
    26 => {
      _match_pattern = 3
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_start + 1
      break
    }
    27 => {
      _match_pattern = 40
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_end
      continue match lexbuf.next_as_int() {
          48..=57 => 27
          65..=90 => 27
          95 => 27
          97..=122 => 27
          _ => break
        }
    }
    28 => {
      _match_pattern = 30
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_start + 1
      continue match lexbuf.next_as_int() {
          60 => 38
          _ => break
        }
    }
    29 => {
      _match_pattern = 5
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_start + 1
      continue match lexbuf.next_as_int() {
          33 => 6
          36..=38 => 6
          42..=43 => 6
          45..=47 => 6
          58 => 6
          60..=64 => 6
          93 => 46
          94 => 6
          124 => 47
          126 => 6
          _ => break
        }
    }
    30 => {
      _match_pattern = 56
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_start + 1
      break
    }
    31 => {
      _match_pattern = 65
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_start + 1
      break
    }
    32 => {
      _match_pattern = 60
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_start + 2
      break
    }
    33 => {
      _match_pattern = 36
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_start + 2
      continue match lexbuf.next_as_int() {
          37 => 50
          _ => break
        }
    }
    34 => {
      _match_pattern = 34
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_start + 2
      break
    }
    35 => {
      _match_pattern = 35
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_start + 2
      break
    }
    36 => {
      _match_pattern = 42
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_start + 2
      continue match lexbuf.next_as_int() {
          64 => 49
          _ => break
        }
    }
    37 => {
      _match_pattern = 33
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_start + 2
      break
    }
    38 => {
      _match_pattern = 31
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_start + 2
      break
    }
    39 => {
      _match_pattern = 21
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_start + 2
      break
    }
    40 => {
      _match_pattern = 20
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_start + 2
      break
    }
    41 => {
      _match_pattern = 15
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_start + 2
      break
    }
    42 => {
      _match_pattern = 10
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_start + 2
      break
    }
    43 => {
      _match_pattern = 11
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_start + 2
      break
    }
    44 => {
      _match_pattern = 12
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_start + 2
      break
    }
    45 =>
      continue match lexbuf.next_as_int() {
          111 => 51
          _ => break
        }
    46 => {
      _match_pattern = 7
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_start + 2
      break
    }
    47 => {
      _match_pattern = 6
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_start + 2
      continue match lexbuf.next_as_int() {
          33 => 6
          36..=38 => 6
          42..=43 => 6
          45..=47 => 6
          58 => 6
          60..=64 => 6
          94 => 6
          124 => 6
          126 => 6
          _ => break
        }
    }
    48 => {
      _match_pattern = 1
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_start + 2
      continue match lexbuf.next_as_int() {
          33 => 6
          36..=38 => 6
          42..=43 => 6
          45..=47 => 6
          58 => 6
          60..=64 => 6
          94 => 6
          124 => 6
          126 => 6
          _ => break
        }
    }
    49 => {
      _match_pattern = 43
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_start + 3
      continue match lexbuf.next_as_int() {
          64 => 52
          _ => break
        }
    }
    50 => {
      _match_pattern = 37
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_start + 3
      break
    }
    51 =>
      continue match lexbuf.next_as_int() {
          100 => 53
          _ => break
        }
    52 => {
      _match_pattern = 44
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_start + 4
      break
    }
    53 =>
      continue match lexbuf.next_as_int() {
          111 => 54
          _ => break
        }
    54 =>
      continue match lexbuf.next_as_int() {
          49 => 55
          51 => 56
          56 => 57
          57 => 58
          _ => break
        }
    55 =>
      continue match lexbuf.next_as_int() {
          49 => 62
          51 => 63
          52 => 64
          53 => 65
          54 => 66
          62 => 67
          _ => break
        }
    56 =>
      continue match lexbuf.next_as_int() {
          62 => 61
          _ => break
        }
    57 =>
      continue match lexbuf.next_as_int() {
          62 => 60
          _ => break
        }
    58 =>
      continue match lexbuf.next_as_int() {
          62 => 59
          _ => break
        }
    59 => {
      _match_pattern = 27
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_start + 7
      break
    }
    60 => {
      _match_pattern = 26
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_start + 7
      break
    }
    61 => {
      _match_pattern = 18
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_start + 7
      break
    }
    62 =>
      continue match lexbuf.next_as_int() {
          62 => 72
          _ => break
        }
    63 =>
      continue match lexbuf.next_as_int() {
          62 => 71
          _ => break
        }
    64 =>
      continue match lexbuf.next_as_int() {
          62 => 70
          _ => break
        }
    65 =>
      continue match lexbuf.next_as_int() {
          62 => 69
          _ => break
        }
    66 =>
      continue match lexbuf.next_as_int() {
          62 => 68
          _ => break
        }
    67 => {
      _match_pattern = 8
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_start + 7
      break
    }
    68 => {
      _match_pattern = 64
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_start + 8
      break
    }
    69 => {
      _match_pattern = 62
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_start + 8
      break
    }
    70 => {
      _match_pattern = 53
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_start + 8
      break
    }
    71 => {
      _match_pattern = 48
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_start + 8
      break
    }
    72 => {
      _match_pattern = 29
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_start + 8
      break
    }
    _ => panic()
  }
  guard _match_pattern <= 67 else {
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
      (AMPERAMPER, _capture_0_start, _capture_0_end)
    }
    2 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      (AMPERSAND, _capture_0_start, _capture_0_end)
    }
    3 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      (BACKQUOTE, _capture_0_start, _capture_0_end)
    }
    4 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      (BANG, _capture_0_start, _capture_0_end)
    }
    5 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      (BAR, _capture_0_start, _capture_0_end)
    }
    6 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      (BARBAR, _capture_0_start, _capture_0_end)
    }
    7 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      (BARRBRACKET, _capture_0_start, _capture_0_end)
    }
    8 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      (CHAR, _capture_0_start, _capture_0_end)
    }
    9 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      (COLON, _capture_0_start, _capture_0_end)
    }
    10 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      (COLONCOLON, _capture_0_start, _capture_0_end)
    }
    11 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      (COLONEQUAL, _capture_0_start, _capture_0_end)
    }
    12 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      (COLONGREATER, _capture_0_start, _capture_0_end)
    }
    13 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      (COMMA, _capture_0_start, _capture_0_end)
    }
    14 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      (DOT, _capture_0_start, _capture_0_end)
    }
    15 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      (DOTDOT, _capture_0_start, _capture_0_end)
    }
    16 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      (EOF, _capture_0_start, _capture_0_end)
    }
    17 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      (EQUAL, _capture_0_start, _capture_0_end)
    }
    18 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      (FLOAT, _capture_0_start, _capture_0_end)
    }
    19 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      (GREATER, _capture_0_start, _capture_0_end)
    }
    20 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      (GREATERRBRACE, _capture_0_start, _capture_0_end)
    }
    21 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      (GREATERRBRACKET, _capture_0_start, _capture_0_end)
    }
    22 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      (INFIXOP0, _capture_0_start, _capture_0_end)
    }
    23 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      (INFIXOP1, _capture_0_start, _capture_0_end)
    }
    24 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      (INFIXOP2, _capture_0_start, _capture_0_end)
    }
    25 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      (INFIXOP3, _capture_0_start, _capture_0_end)
    }
    26 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      (INFIXOP4, _capture_0_start, _capture_0_end)
    }
    27 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      (DOTOP, _capture_0_start, _capture_0_end)
    }
    28 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      (INT, _capture_0_start, _capture_0_end)
    }
    29 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      (LABEL, _capture_0_start, _capture_0_end)
    }
    30 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      (LBRACE, _capture_0_start, _capture_0_end)
    }
    31 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      (LBRACELESS, _capture_0_start, _capture_0_end)
    }
    32 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      (LBRACKET, _capture_0_start, _capture_0_end)
    }
    33 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      (LBRACKETBAR, _capture_0_start, _capture_0_end)
    }
    34 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      (LBRACKETLESS, _capture_0_start, _capture_0_end)
    }
    35 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      (LBRACKETGREATER, _capture_0_start, _capture_0_end)
    }
    36 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      (LBRACKETPERCENT, _capture_0_start, _capture_0_end)
    }
    37 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      (LBRACKETPERCENTPERCENT, _capture_0_start, _capture_0_end)
    }
    38 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      (LESS, _capture_0_start, _capture_0_end)
    }
    39 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      (LESSMINUS, _capture_0_start, _capture_0_end)
    }
    40 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      (all_keywords.get(t).unwrap_or(LIDENT), _capture_0_start, _capture_0_end)
    }
    41 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      (LPAREN, _capture_0_start, _capture_0_end)
    }
    42 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      (LBRACKETAT, _capture_0_start, _capture_0_end)
    }
    43 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      (LBRACKETATAT, _capture_0_start, _capture_0_end)
    }
    44 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      (LBRACKETATATAT, _capture_0_start, _capture_0_end)
    }
    45 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      (MINUS, _capture_0_start, _capture_0_end)
    }
    46 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      (MINUSDOT, _capture_0_start, _capture_0_end)
    }
    47 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      (MINUSGREATER, _capture_0_start, _capture_0_end)
    }
    48 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      (OPTLABEL, _capture_0_start, _capture_0_end)
    }
    49 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      (PERCENT, _capture_0_start, _capture_0_end)
    }
    50 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      (PLUS, _capture_0_start, _capture_0_end)
    }
    51 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      (PLUSDOT, _capture_0_start, _capture_0_end)
    }
    52 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      (PLUSEQ, _capture_0_start, _capture_0_end)
    }
    53 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      (PREFIXOP, _capture_0_start, _capture_0_end)
    }
    54 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      (QUESTION, _capture_0_start, _capture_0_end)
    }
    55 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      (QUOTE, _capture_0_start, _capture_0_end)
    }
    56 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      (RBRACE, _capture_0_start, _capture_0_end)
    }
    57 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      (RBRACKET, _capture_0_start, _capture_0_end)
    }
    58 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      (RPAREN, _capture_0_start, _capture_0_end)
    }
    59 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      (SEMI, _capture_0_start, _capture_0_end)
    }
    60 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      (SEMISEMI, _capture_0_start, _capture_0_end)
    }
    61 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      (HASH, _capture_0_start, _capture_0_end)
    }
    62 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      (HASHOP, _capture_0_start, _capture_0_end)
    }
    63 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      (STAR, _capture_0_start, _capture_0_end)
    }
    64 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      (STRING, _capture_0_start, _capture_0_end)
    }
    65 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      (TILDE, _capture_0_start, _capture_0_end)
    }
    66 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      (UIDENT, _capture_0_start, _capture_0_end)
    }
    67 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      (UNDERSCORE, _capture_0_start, _capture_0_end)
    }
    _ => panic()
  }
}

///|
pub fn new_lexer(input : String) -> Lexer {
  @lexbuf.StringLexbuf::from_string(input)
}

///|
pub fn Lexer::next_token(self : Lexer) -> (Token, Int, Int) {
  token(self.0)
}
