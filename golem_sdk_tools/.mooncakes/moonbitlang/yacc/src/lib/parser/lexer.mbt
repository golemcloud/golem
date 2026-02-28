///|
pub suberror LexError {
  UnexpectedEndOfFile
  Unrecognized(Char, Int)
}

///|
struct Lexer {
  buf : @lexbuf.StringLexbuf
  // 0 %%
  // %% 1
  // %% %% 2
  phase : Ref[Int]
}

///|
fn token(
  phase : Ref[Int],
  lexbuf : @lexbuf.StringLexbuf,
) -> (Token, Int, Int) raise LexError {
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
          33 => 2
          34 => 4
          35..=36 => 2
          37 => 5
          38..=39 => 2
          40 => 6
          41 => 7
          42..=43 => 2
          44 => 8
          45 => 9
          46 => 2
          47 => 10
          48..=57 => 2
          58 => 11
          59 => 12
          60 => 13
          61 => 14
          62 => 2
          63 => 15
          64 => 16
          65..=90 => 17
          91 => 18
          92 => 2
          93 => 19
          94 => 2
          95 => 17
          96 => 2
          97..=122 => 17
          123 => 20
          124 => 21
          125..=1114111 => 2
          _ => break
        }
    1 => {
      _match_pattern = 34
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_end
      break
    }
    2 => {
      _match_pattern = 33
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
      _match_pattern = 33
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_start + 1
      continue match lexbuf.next_as_int() {
          0..=33 => 22
          35..=1114111 => 22
          _ => break
        }
    }
    5 => {
      _match_pattern = 33
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_start + 1
      continue match lexbuf.next_as_int() {
          37 => 24
          100 => 25
          105 => 26
          108 => 27
          110 => 28
          112 => 29
          114 => 30
          115 => 31
          116 => 32
          123 => 33
          _ => break
        }
    }
    6 => {
      _match_pattern = 27
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_start + 1
      continue match lexbuf.next_as_int() {
          42 => 37
          _ => break
        }
    }
    7 => {
      _match_pattern = 28
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_start + 1
      break
    }
    8 => {
      _match_pattern = 29
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_start + 1
      break
    }
    9 => {
      _match_pattern = 33
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_start + 1
      continue match lexbuf.next_as_int() {
          62 => 23
          _ => break
        }
    }
    10 => {
      _match_pattern = 33
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_start + 1
      continue match lexbuf.next_as_int() {
          42 => 35
          47 => 36
          _ => break
        }
    }
    11 => {
      _match_pattern = 19
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_start + 1
      break
    }
    12 => {
      _match_pattern = 22
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_start + 1
      break
    }
    13 => {
      _match_pattern = 24
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_start + 1
      break
    }
    14 => {
      _match_pattern = 21
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_start + 1
      break
    }
    15 => {
      _match_pattern = 30
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_start + 1
      break
    }
    16 => {
      _match_pattern = 33
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_start + 1
      continue match lexbuf.next_as_int() {
          65..=90 => 34
          95 => 34
          97..=122 => 34
          _ => break
        }
    }
    17 => {
      _match_pattern = 6
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_end
      continue match lexbuf.next_as_int() {
          48..=57 => 17
          65..=90 => 17
          95 => 17
          97..=122 => 17
          _ => break
        }
    }
    18 => {
      _match_pattern = 25
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_start + 1
      break
    }
    19 => {
      _match_pattern = 26
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_start + 1
      break
    }
    20 => {
      _match_pattern = 31
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_start + 1
      break
    }
    21 => {
      _match_pattern = 20
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_start + 1
      break
    }
    22 =>
      continue match lexbuf.next_as_int() {
          0..=33 => 22
          34 => 38
          35..=1114111 => 22
          _ => break
        }
    23 => {
      _match_pattern = 18
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_start + 2
      break
    }
    24 => {
      _match_pattern = 17
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_start + 2
      break
    }
    25 =>
      continue match lexbuf.next_as_int() {
          101 => 40
          _ => break
        }
    26 =>
      continue match lexbuf.next_as_int() {
          110 => 39
          _ => break
        }
    27 =>
      continue match lexbuf.next_as_int() {
          101 => 43
          _ => break
        }
    28 =>
      continue match lexbuf.next_as_int() {
          111 => 41
          _ => break
        }
    29 =>
      continue match lexbuf.next_as_int() {
          111 => 44
          114 => 45
          _ => break
        }
    30 =>
      continue match lexbuf.next_as_int() {
          105 => 42
          _ => break
        }
    31 =>
      continue match lexbuf.next_as_int() {
          116 => 48
          _ => break
        }
    32 =>
      continue match lexbuf.next_as_int() {
          111 => 46
          121 => 47
          _ => break
        }
    33 => {
      _match_pattern = 23
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_start + 2
      break
    }
    34 => {
      _tag_0 = lexbuf.curr_pos()
      continue match lexbuf.next_as_int() {
          46 => 60
          47 => 62
          48..=57 => 34
          65..=90 => 34
          95 => 34
          97..=122 => 34
          _ => break
        }
    }
    35 =>
      continue match lexbuf.next_as_int() {
          0..=41 => 35
          42 => 65
          43..=46 => 35
          47 => 64
          48..=1114111 => 35
          _ => break
        }
    36 => {
      _match_pattern = 4
      _match_end = lexbuf.curr_pos()
      continue match lexbuf.next_as_int() {
          0..=9 => 36
          10 => 63
          11..=1114111 => 36
          _ => break
        }
    }
    37 => {
      _match_pattern = 2
      _match_end = lexbuf.curr_pos()
      continue match lexbuf.next_as_int() {
          41 => 49
          _ => break
        }
    }
    38 => {
      _match_pattern = 32
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_end
      break
    }
    39 =>
      continue match lexbuf.next_as_int() {
          108 => 50
          _ => break
        }
    40 =>
      continue match lexbuf.next_as_int() {
          114 => 51
          _ => break
        }
    41 =>
      continue match lexbuf.next_as_int() {
          110 => 52
          _ => break
        }
    42 =>
      continue match lexbuf.next_as_int() {
          103 => 53
          _ => break
        }
    43 =>
      continue match lexbuf.next_as_int() {
          102 => 54
          _ => break
        }
    44 =>
      continue match lexbuf.next_as_int() {
          115 => 56
          _ => break
        }
    45 =>
      continue match lexbuf.next_as_int() {
          101 => 55
          _ => break
        }
    46 =>
      continue match lexbuf.next_as_int() {
          107 => 58
          _ => break
        }
    47 =>
      continue match lexbuf.next_as_int() {
          112 => 57
          _ => break
        }
    48 =>
      continue match lexbuf.next_as_int() {
          97 => 59
          _ => break
        }
    49 => {
      _match_pattern = 1
      _match_end = lexbuf.curr_pos()
      break
    }
    50 =>
      continue match lexbuf.next_as_int() {
          105 => 67
          _ => break
        }
    51 =>
      continue match lexbuf.next_as_int() {
          105 => 68
          _ => break
        }
    52 =>
      continue match lexbuf.next_as_int() {
          97 => 69
          _ => break
        }
    53 =>
      continue match lexbuf.next_as_int() {
          104 => 70
          _ => break
        }
    54 =>
      continue match lexbuf.next_as_int() {
          116 => 71
          _ => break
        }
    55 =>
      continue match lexbuf.next_as_int() {
          99 => 72
          _ => break
        }
    56 =>
      continue match lexbuf.next_as_int() {
          105 => 73
          _ => break
        }
    57 =>
      continue match lexbuf.next_as_int() {
          101 => 74
          _ => break
        }
    58 =>
      continue match lexbuf.next_as_int() {
          101 => 75
          _ => break
        }
    59 =>
      continue match lexbuf.next_as_int() {
          114 => 76
          _ => break
        }
    60 => {
      _tag_1 = lexbuf.curr_pos()
      continue match lexbuf.next_as_int() {
          65..=90 => 61
          95 => 61
          97..=122 => 61
          _ => break
        }
    }
    61 => {
      _match_pattern = 5
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_end
      _capture_1_start = _tag_1
      _capture_1_end = _match_end
      _capture_2_start = _match_start + 1
      _capture_2_end = _tag_0
      continue match lexbuf.next_as_int() {
          48..=57 => 61
          65..=90 => 61
          95 => 61
          97..=122 => 61
          _ => break
        }
    }
    62 =>
      continue match lexbuf.next_as_int() {
          65..=90 => 34
          95 => 34
          97..=122 => 34
          _ => break
        }
    63 => {
      _match_pattern = 4
      _match_end = lexbuf.curr_pos()
      break
    }
    64 =>
      continue match lexbuf.next_as_int() {
          0..=41 => 35
          43..=1114111 => 35
          _ => break
        }
    65 =>
      continue match lexbuf.next_as_int() {
          0..=41 => 35
          42 => 65
          43..=46 => 35
          47 => 66
          48..=1114111 => 35
          _ => break
        }
    66 => {
      _match_pattern = 3
      _match_end = lexbuf.curr_pos()
      continue match lexbuf.next_as_int() {
          0..=41 => 35
          43..=1114111 => 35
          _ => break
        }
    }
    67 =>
      continue match lexbuf.next_as_int() {
          110 => 77
          _ => break
        }
    68 =>
      continue match lexbuf.next_as_int() {
          118 => 78
          _ => break
        }
    69 =>
      continue match lexbuf.next_as_int() {
          115 => 79
          _ => break
        }
    70 =>
      continue match lexbuf.next_as_int() {
          116 => 80
          _ => break
        }
    71 => {
      _match_pattern = 11
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_start + 5
      break
    }
    72 => {
      _match_pattern = 14
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_start + 5
      break
    }
    73 =>
      continue match lexbuf.next_as_int() {
          116 => 81
          _ => break
        }
    74 => {
      _match_pattern = 9
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_start + 5
      break
    }
    75 =>
      continue match lexbuf.next_as_int() {
          110 => 82
          _ => break
        }
    76 =>
      continue match lexbuf.next_as_int() {
          116 => 83
          _ => break
        }
    77 =>
      continue match lexbuf.next_as_int() {
          101 => 84
          _ => break
        }
    78 =>
      continue match lexbuf.next_as_int() {
          101 => 85
          _ => break
        }
    79 =>
      continue match lexbuf.next_as_int() {
          115 => 86
          _ => break
        }
    80 => {
      _match_pattern = 12
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_start + 6
      break
    }
    81 =>
      continue match lexbuf.next_as_int() {
          105 => 87
          _ => break
        }
    82 => {
      _match_pattern = 8
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_start + 6
      break
    }
    83 => {
      _match_pattern = 7
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_start + 6
      break
    }
    84 => {
      _match_pattern = 16
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_start + 7
      break
    }
    85 => {
      _match_pattern = 15
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_start + 7
      break
    }
    86 =>
      continue match lexbuf.next_as_int() {
          111 => 88
          _ => break
        }
    87 =>
      continue match lexbuf.next_as_int() {
          111 => 89
          _ => break
        }
    88 =>
      continue match lexbuf.next_as_int() {
          99 => 90
          _ => break
        }
    89 =>
      continue match lexbuf.next_as_int() {
          110 => 91
          _ => break
        }
    90 => {
      _match_pattern = 13
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_start + 9
      break
    }
    91 => {
      _match_pattern = 10
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_start + 9
      break
    }
    _ => panic()
  }
  guard _match_pattern <= 34 else {
    // No pattern matched
    panic()
  }
  lexbuf.reset(pos=_match_end)
  match _match_pattern {
    0 => {
      ()
      token(phase, lexbuf)
    }
    1 => {
      ()
      token(phase, lexbuf)
    }
    2 => {
      ()
      comment_star_rparen(lexbuf)
      token(phase, lexbuf)
    }
    3 => {
      ()
      token(phase, lexbuf)
    }
    4 => {
      ()
      token(phase, lexbuf)
    }
    5 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      let id = lexbuf.get_string(_capture_1_start, _capture_1_end)
      let pkg = lexbuf.get_string(_capture_2_start, _capture_2_end)
      (PKG_AND_IDENT((pkg, id)), _capture_0_start, _capture_0_end)
    }
    6 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      (IDENT(t), _capture_0_start, _capture_0_end)
    }
    7 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      (PERCENT_START, _capture_0_start, _capture_0_end)
    }
    8 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      (PERCENT_TOKEN, _capture_0_start, _capture_0_end)
    }
    9 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      (PERCENT_TYPE, _capture_0_start, _capture_0_end)
    }
    10 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      (PERCENT_POSITION, _capture_0_start, _capture_0_end)
    }
    11 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      (PERCENT_LEFT, _capture_0_start, _capture_0_end)
    }
    12 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      (PERCENT_RIGHT, _capture_0_start, _capture_0_end)
    }
    13 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      (PERCENT_NONASSOC, _capture_0_start, _capture_0_end)
    }
    14 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      (PERCENT_PREC, _capture_0_start, _capture_0_end)
    }
    15 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      (PERCENT_DERIVE, _capture_0_start, _capture_0_end)
    }
    16 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      (PERCENT_INLINE, _capture_0_start, _capture_0_end)
    }
    17 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      if phase.val <= 0 {
        phase.val += 1
        (PERCENT_PERCENT, _capture_0_start, _capture_0_end)
      } else {
        phase.val += 1
        let sb = StringBuilder::new()
        code_eof(sb, lexbuf)
        (
          PERCENT_PERCENT_CODE_EOF(
            (sb.to_string(), _capture_0_end, lexbuf.pos - _capture_0_end),
          ),
          _capture_0_start,
          lexbuf.pos,
        )
      }
    }
    18 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      (ARROW, _capture_0_start, _capture_0_end)
    }
    19 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      (COLON, _capture_0_start, _capture_0_end)
    }
    20 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      (BAR, _capture_0_start, _capture_0_end)
    }
    21 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      (EQ, _capture_0_start, _capture_0_end)
    }
    22 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      (SEMI, _capture_0_start, _capture_0_end)
    }
    23 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      let sb = StringBuilder::new()
      code_percent_rbrace(sb, lexbuf)
      (
        PERCENT_LBRACE_CODE_PERCENT_RBRACE(
          (sb.to_string(), _capture_0_end, lexbuf.pos - _capture_0_end),
        ),
        _capture_0_start,
        lexbuf.pos,
      )
    }
    24 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      let sb = StringBuilder::new()
      code_rangle(sb, lexbuf)
      (LANGLE_CODE_RANGLE(sb.to_string()), _capture_0_start, lexbuf.pos)
    }
    25 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      (LBRACKET, _capture_0_start, _capture_0_end)
    }
    26 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      (RBRACKET, _capture_0_start, _capture_0_end)
    }
    27 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      (LPAREN, _capture_0_start, _capture_0_end)
    }
    28 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      (RPAREN, _capture_0_start, _capture_0_end)
    }
    29 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      (COMMA, _capture_0_start, _capture_0_end)
    }
    30 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      (QUESTION, _capture_0_start, _capture_0_end)
    }
    31 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      let sb = StringBuilder::new()
      let subst = []
      let base = lexbuf.pos
      code_rbrace(sb, subst, base, lexbuf)
      (
        LBRACE_CODE_RBRACE((sb.to_string(), base, subst)),
        _capture_0_start,
        lexbuf.pos,
      )
    }
    32 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      (STRING(t), _capture_0_start, _capture_0_end)
    }
    33 => {
      ()
      let t = lexbuf.get_char(_capture_0_start, _capture_0_end)
      let _ = t
      raise Unrecognized(t, _capture_0_start)
    }
    34 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      (EOF, _capture_0_start, _capture_0_end)
    }
    _ => panic()
  }
}

///|
fn comment_star_rparen(lexbuf : @lexbuf.StringLexbuf) -> Unit raise LexError {
  // The matched pattern id
  let mut _match_pattern = @int.max_value
  let mut _match_start = lexbuf.curr_pos()
  let mut _match_end = -1
  let mut _capture_0_start = -1
  let mut _capture_0_end = -1
  loop 0 {
    0 =>
      continue match lexbuf.next_as_int() {
          -1 => 1
          0..=39 => 2
          40 => 3
          41 => 2
          42 => 4
          43..=1114111 => 2
          _ => break
        }
    1 => {
      _match_pattern = 3
      _match_end = lexbuf.curr_pos()
      break
    }
    2 => {
      _match_pattern = 2
      _match_end = lexbuf.curr_pos()
      break
    }
    3 => {
      _match_pattern = 2
      _match_end = lexbuf.curr_pos()
      continue match lexbuf.next_as_int() {
          42 => 6
          _ => break
        }
    }
    4 => {
      _match_pattern = 2
      _match_end = lexbuf.curr_pos()
      continue match lexbuf.next_as_int() {
          41 => 5
          _ => break
        }
    }
    5 => {
      _match_pattern = 1
      _match_end = lexbuf.curr_pos()
      break
    }
    6 => {
      _match_pattern = 0
      _match_end = lexbuf.curr_pos()
      break
    }
    _ => panic()
  }
  guard _match_pattern <= 3 else {
    // No pattern matched
    panic()
  }
  lexbuf.reset(pos=_match_end)
  match _match_pattern {
    0 => {
      ()
      comment_star_rparen(lexbuf)
      comment_star_rparen(lexbuf)
    }
    1 => {
      ()
      ()
    }
    2 => {
      ()
      comment_star_rparen(lexbuf)
    }
    3 => {
      ()
      raise UnexpectedEndOfFile
    }
    _ => panic()
  }
}

///|
fn code_eof(sb : StringBuilder, lexbuf : @lexbuf.StringLexbuf) -> Unit {
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
          0..=1114111 => 2
          _ => break
        }
    1 => {
      _match_pattern = 1
      _match_end = lexbuf.curr_pos()
      break
    }
    2 => {
      _match_pattern = 0
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_start + 1
      break
    }
    _ => panic()
  }
  guard _match_pattern <= 1 else {
    // No pattern matched
    panic()
  }
  lexbuf.reset(pos=_match_end)
  match _match_pattern {
    0 => {
      ()
      let t = lexbuf.get_char(_capture_0_start, _capture_0_end)
      sb.write_char(t)
      code_eof(sb, lexbuf)
    }
    1 => {
      ()
      ()
    }
    _ => panic()
  }
}

///|
fn code_rangle(
  sb : StringBuilder,
  lexbuf : @lexbuf.StringLexbuf,
) -> Unit raise LexError {
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
          0..=61 => 2
          62 => 3
          63..=1114111 => 2
          _ => break
        }
    1 => {
      _match_pattern = 2
      _match_end = lexbuf.curr_pos()
      break
    }
    2 => {
      _match_pattern = 1
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_start + 1
      break
    }
    3 => {
      _match_pattern = 0
      _match_end = lexbuf.curr_pos()
      break
    }
    _ => panic()
  }
  guard _match_pattern <= 2 else {
    // No pattern matched
    panic()
  }
  lexbuf.reset(pos=_match_end)
  match _match_pattern {
    0 => {
      ()
      ()
    }
    1 => {
      ()
      let t = lexbuf.get_char(_capture_0_start, _capture_0_end)
      sb.write_char(t)
      code_rangle(sb, lexbuf)
    }
    2 => {
      ()
      raise UnexpectedEndOfFile
    }
    _ => panic()
  }
}

///|
fn code_percent_rbrace(
  sb : StringBuilder,
  lexbuf : @lexbuf.StringLexbuf,
) -> Unit raise LexError {
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
          0..=36 => 2
          37 => 3
          38..=1114111 => 2
          _ => break
        }
    1 => {
      _match_pattern = 2
      _match_end = lexbuf.curr_pos()
      break
    }
    2 => {
      _match_pattern = 1
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_start + 1
      break
    }
    3 => {
      _match_pattern = 1
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_start + 1
      continue match lexbuf.next_as_int() {
          125 => 4
          _ => break
        }
    }
    4 => {
      _match_pattern = 0
      _match_end = lexbuf.curr_pos()
      break
    }
    _ => panic()
  }
  guard _match_pattern <= 2 else {
    // No pattern matched
    panic()
  }
  lexbuf.reset(pos=_match_end)
  match _match_pattern {
    0 => {
      ()
      ()
    }
    1 => {
      ()
      let t = lexbuf.get_char(_capture_0_start, _capture_0_end)
      sb.write_char(t)
      code_percent_rbrace(sb, lexbuf)
    }
    2 => {
      ()
      raise UnexpectedEndOfFile
    }
    _ => panic()
  }
}

///|
fn code_rbrace(
  sb : StringBuilder,
  subst : Array[@ast.SubstItem],
  base : Int,
  lexbuf : @lexbuf.StringLexbuf,
) -> Unit raise LexError {
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
  let mut _tag_2 = -1
  loop 0 {
    0 =>
      continue match lexbuf.next_as_int() {
          -1 => 1
          0..=35 => 2
          36 => 3
          37..=122 => 2
          123 => 4
          124 => 2
          125 => 5
          126..=1114111 => 2
          _ => break
        }
    1 => {
      _match_pattern = 7
      _match_end = lexbuf.curr_pos()
      break
    }
    2 => {
      _match_pattern = 6
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_start + 1
      break
    }
    3 => {
      _match_pattern = 6
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_start + 1
      continue match lexbuf.next_as_int() {
          48..=57 => 6
          65..=90 => 7
          95 => 7
          97..=100 => 7
          101 => 8
          102..=107 => 7
          108 => 9
          109..=114 => 7
          115 => 10
          116..=122 => 7
          _ => break
        }
    }
    4 => {
      _match_pattern = 0
      _match_end = lexbuf.curr_pos()
      break
    }
    5 => {
      _match_pattern = 1
      _match_end = lexbuf.curr_pos()
      break
    }
    6 => {
      _match_pattern = 2
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_end
      _capture_1_start = _match_start + 1
      _capture_1_end = _match_end
      continue match lexbuf.next_as_int() {
          48..=57 => 6
          _ => break
        }
    }
    7 => {
      _match_pattern = 5
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_end
      continue match lexbuf.next_as_int() {
          48..=57 => 7
          65..=90 => 7
          95 => 7
          97..=122 => 7
          _ => break
        }
    }
    8 => {
      _match_pattern = 5
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_end
      continue match lexbuf.next_as_int() {
          48..=57 => 7
          65..=90 => 7
          95 => 7
          97..=109 => 7
          110 => 12
          111..=122 => 7
          _ => break
        }
    }
    9 => {
      _match_pattern = 5
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_end
      continue match lexbuf.next_as_int() {
          48..=57 => 7
          65..=90 => 7
          95 => 7
          97..=110 => 7
          111 => 11
          112..=122 => 7
          _ => break
        }
    }
    10 => {
      _match_pattern = 5
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_end
      continue match lexbuf.next_as_int() {
          48..=57 => 7
          65..=90 => 7
          95 => 7
          97..=107 => 7
          108 => 13
          109..=115 => 7
          116 => 14
          117..=120 => 7
          121 => 15
          122 => 7
          _ => break
        }
    }
    11 => {
      _match_pattern = 5
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_end
      continue match lexbuf.next_as_int() {
          48..=57 => 7
          65..=90 => 7
          95 => 7
          97..=98 => 7
          99 => 16
          100..=122 => 7
          _ => break
        }
    }
    12 => {
      _match_pattern = 5
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_end
      continue match lexbuf.next_as_int() {
          48..=57 => 7
          65..=90 => 7
          95 => 7
          97..=99 => 7
          100 => 17
          101..=122 => 7
          _ => break
        }
    }
    13 => {
      _match_pattern = 5
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_end
      continue match lexbuf.next_as_int() {
          48..=57 => 7
          65..=90 => 7
          95 => 7
          97..=110 => 7
          111 => 18
          112..=122 => 7
          _ => break
        }
    }
    14 => {
      _match_pattern = 5
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_end
      continue match lexbuf.next_as_int() {
          48..=57 => 7
          65..=90 => 7
          95 => 7
          97 => 20
          98..=122 => 7
          _ => break
        }
    }
    15 => {
      _match_pattern = 5
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_end
      continue match lexbuf.next_as_int() {
          48..=57 => 7
          65..=90 => 7
          95 => 7
          97..=108 => 7
          109 => 19
          110..=122 => 7
          _ => break
        }
    }
    16 => {
      _tag_0 = lexbuf.curr_pos()
      _match_pattern = 4
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_end
      _capture_1_start = _match_start + 1
      _capture_1_end = _match_end
      continue match lexbuf.next_as_int() {
          40 => 38
          48..=57 => 7
          65..=90 => 7
          95 => 7
          97..=122 => 7
          _ => break
        }
    }
    17 => {
      _match_pattern = 5
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_end
      continue match lexbuf.next_as_int() {
          48..=57 => 7
          65..=90 => 7
          95 => 7
          97..=111 => 7
          112 => 37
          113..=122 => 7
          _ => break
        }
    }
    18 => {
      _match_pattern = 5
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_end
      continue match lexbuf.next_as_int() {
          48..=57 => 7
          65..=90 => 7
          95 => 7
          97..=98 => 7
          99 => 21
          100..=122 => 7
          _ => break
        }
    }
    19 => {
      _match_pattern = 5
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_end
      continue match lexbuf.next_as_int() {
          48..=57 => 7
          65..=90 => 7
          95 => 7
          97 => 7
          98 => 22
          99..=122 => 7
          _ => break
        }
    }
    20 => {
      _match_pattern = 5
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_end
      continue match lexbuf.next_as_int() {
          48..=57 => 7
          65..=90 => 7
          95 => 7
          97..=113 => 7
          114 => 23
          115..=122 => 7
          _ => break
        }
    }
    21 => {
      _match_pattern = 4
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_end
      _capture_1_start = _match_start + 1
      _capture_1_end = _match_end
      continue match lexbuf.next_as_int() {
          48..=57 => 7
          65..=90 => 7
          95 => 7
          97..=122 => 7
          _ => break
        }
    }
    22 => {
      _match_pattern = 5
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_end
      continue match lexbuf.next_as_int() {
          48..=57 => 7
          65..=90 => 7
          95 => 7
          97..=110 => 7
          111 => 24
          112..=122 => 7
          _ => break
        }
    }
    23 => {
      _match_pattern = 5
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_end
      continue match lexbuf.next_as_int() {
          48..=57 => 7
          65..=90 => 7
          95 => 7
          97..=115 => 7
          116 => 17
          117..=122 => 7
          _ => break
        }
    }
    24 => {
      _match_pattern = 5
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_end
      continue match lexbuf.next_as_int() {
          48..=57 => 7
          65..=90 => 7
          95 => 7
          97..=107 => 7
          108 => 27
          109..=122 => 7
          _ => break
        }
    }
    25 =>
      continue match lexbuf.next_as_int() {
          48..=57 => 26
          _ => break
        }
    26 => {
      _tag_2 = lexbuf.curr_pos()
      continue match lexbuf.next_as_int() {
          9 => 29
          32 => 29
          41 => 28
          48..=57 => 26
          _ => break
        }
    }
    27 => {
      _match_pattern = 5
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_end
      continue match lexbuf.next_as_int() {
          48..=57 => 7
          65..=90 => 7
          95 => 7
          97..=114 => 7
          115 => 30
          116..=122 => 7
          _ => break
        }
    }
    28 => {
      _match_pattern = 3
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_end
      _capture_1_start = _match_start + 1
      _capture_1_end = _tag_0
      _capture_2_start = _tag_1
      _capture_2_end = _tag_2
      break
    }
    29 =>
      continue match lexbuf.next_as_int() {
          9 => 29
          32 => 29
          41 => 28
          _ => break
        }
    30 => {
      _match_pattern = 5
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_end
      continue match lexbuf.next_as_int() {
          48..=57 => 7
          65..=90 => 7
          95 => 7
          97..=115 => 7
          116 => 31
          117..=122 => 7
          _ => break
        }
    }
    31 => {
      _match_pattern = 5
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_end
      continue match lexbuf.next_as_int() {
          48..=57 => 7
          65..=90 => 7
          95 => 7
          97 => 32
          98..=122 => 7
          _ => break
        }
    }
    32 => {
      _match_pattern = 5
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_end
      continue match lexbuf.next_as_int() {
          48..=57 => 7
          65..=90 => 7
          95 => 7
          97..=113 => 7
          114 => 33
          115..=122 => 7
          _ => break
        }
    }
    33 => {
      _match_pattern = 5
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_end
      continue match lexbuf.next_as_int() {
          48..=57 => 7
          65..=90 => 7
          95 => 7
          97..=115 => 7
          116 => 34
          117..=122 => 7
          _ => break
        }
    }
    34 => {
      _match_pattern = 5
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_end
      continue match lexbuf.next_as_int() {
          48..=57 => 7
          65..=90 => 7
          95 => 7
          97..=111 => 7
          112 => 35
          113..=122 => 7
          _ => break
        }
    }
    35 => {
      _match_pattern = 5
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_end
      continue match lexbuf.next_as_int() {
          48..=57 => 7
          65..=90 => 7
          95 => 7
          97..=110 => 7
          111 => 36
          112..=122 => 7
          _ => break
        }
    }
    36 => {
      _match_pattern = 5
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_end
      continue match lexbuf.next_as_int() {
          48..=57 => 7
          65..=90 => 7
          95 => 7
          97..=114 => 7
          115 => 21
          116..=122 => 7
          _ => break
        }
    }
    37 => {
      _match_pattern = 5
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_end
      continue match lexbuf.next_as_int() {
          48..=57 => 7
          65..=90 => 7
          95 => 7
          97..=110 => 7
          111 => 40
          112..=122 => 7
          _ => break
        }
    }
    38 => {
      _tag_1 = lexbuf.curr_pos()
      continue match lexbuf.next_as_int() {
          9 => 38
          32 => 38
          36 => 25
          65..=90 => 39
          95 => 39
          97..=122 => 39
          _ => break
        }
    }
    39 => {
      _tag_2 = lexbuf.curr_pos()
      continue match lexbuf.next_as_int() {
          9 => 29
          32 => 29
          41 => 28
          48..=57 => 39
          65..=90 => 39
          95 => 39
          97..=122 => 39
          _ => break
        }
    }
    40 => {
      _match_pattern = 5
      _match_end = lexbuf.curr_pos()
      _capture_0_start = _match_start
      _capture_0_end = _match_end
      continue match lexbuf.next_as_int() {
          48..=57 => 7
          65..=90 => 7
          95 => 7
          97..=114 => 7
          115 => 16
          116..=122 => 7
          _ => break
        }
    }
    _ => panic()
  }
  guard _match_pattern <= 7 else {
    // No pattern matched
    panic()
  }
  lexbuf.reset(pos=_match_end)
  match _match_pattern {
    0 => {
      ()
      sb.write_string("{")
      code_rbrace(sb, subst, base, lexbuf)
      sb.write_string("}")
      code_rbrace(sb, subst, base, lexbuf)
    }
    1 => {
      ()
      ()
    }
    2 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      let t1 = lexbuf.get_string(_capture_1_start, _capture_1_end)
      sb.write_string(t)
      let index = @strconv.parse_int(t1, base=10) catch {
        err => abort(Show::to_string(err))
      }
      subst.push({
        start: _capture_0_start - base,
        end: _capture_0_end - base,
        desc: Dollar(index),
      })
      code_rbrace(sb, subst, base, lexbuf)
    }
    3 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      let t1 = lexbuf.get_string(_capture_1_start, _capture_1_end)
      let t2 = lexbuf.get_string(_capture_2_start, _capture_2_end)
      sb.write_string(t)
      let arg : @ast.ClauseItemIdent = if t2.has_prefix("$") {
        let index = @strconv.parse_int(try! t2[1:].to_string(), base=10) catch {
          err => abort(Show::to_string(err))
        }
        Dollar(index)
      } else {
        Name(t2)
      }
      subst.push({
        start: _capture_0_start - base,
        end: _capture_0_end - base,
        desc: match t1 {
          "startpos" => StartPosOf(arg)
          "endpos" => EndPosOf(arg)
          "loc" => LocOf(arg)
          _ => panic()
        },
      })
      code_rbrace(sb, subst, base, lexbuf)
    }
    4 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      let _ = t
      let t1 = lexbuf.get_string(_capture_1_start, _capture_1_end)
      sb.write_string(t)
      subst.push({
        start: _capture_0_start - base,
        end: _capture_0_end - base,
        desc: match t1 {
          "startpos" => StartPos
          "endpos" => EndPos
          "loc" => Loc
          "symbolstartpos" => SymbolStartPos
          "sloc" => Sloc
          _ => panic()
        },
      })
      code_rbrace(sb, subst, base, lexbuf)
    }
    5 => {
      ()
      let t = lexbuf.get_string(_capture_0_start, _capture_0_end)
      sb.write_string(t)
      code_rbrace(sb, subst, base, lexbuf)
    }
    6 => {
      ()
      let t = lexbuf.get_char(_capture_0_start, _capture_0_end)
      sb.write_char(t)
      code_rbrace(sb, subst, base, lexbuf)
    }
    7 => {
      ()
      raise UnexpectedEndOfFile
    }
    _ => panic()
  }
}

///|
pub fn new_lexer(input : String) -> Lexer {
  { buf: @lexbuf.StringLexbuf::from_string(input), phase: { val: 0 } }
}

///|
pub fn Lexer::next_token(self : Lexer) -> (Token, Int, Int) raise LexError {
  token(self.phase, self.buf)
}
