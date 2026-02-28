///|
let use_utf16_location : Ref[Bool] = Ref::new(false)

///|
fn lex_tokens(
  input : StringView,
  base~ : StringView,
  env~ : LexEnv,
  preserve_comment~ : (Comment, Int, Int) -> Unit,
) -> Unit {
  lexmatch input with longest {
    (
      "\n|\r|\r\n|\u2028|\u2029",
      // Handle newlines
      rest
    ) => {
      let start_pos = env.calc_offset(input, base~)
      let end_pos = env.calc_offset(rest, base~)
      env.add_token_with_loc(NEWLINE, start=start_pos, end=end_pos)
      env.current_bol = end_pos
      env.current_line += 1
      lex_tokens(rest, base~, env~, preserve_comment~)
    }

    // Handle whitespace (Unicode spaces)

    (
      "[\u0009\u000B\u000C\u0020\u00A0\uFEFF\u1680\u2000-\u200A\u202F\u205F\u3000]+",
      rest
    ) => lex_tokens(rest, base~, env~, preserve_comment~)

    // Fat arrow
    ("=>", rest) => {
      env.add_token_with_loc(
        FAT_ARROW,
        start=env.calc_offset(input, base~),
        end=env.calc_offset(rest, base~),
      )
      lex_tokens(rest, base~, env~, preserve_comment~)
    }

    // Thin arrow
    ("->", rest) => {
      env.add_token_with_loc(
        THIN_ARROW,
        start=env.calc_offset(input, base~),
        end=env.calc_offset(rest, base~),
      )
      lex_tokens(rest, base~, env~, preserve_comment~)
    }

    // Comments
    ("//[^\r\n]*" as raw, rest) => {
      let start_pos = env.calc_offset(input, base~)
      env.register_surrogate_pair(raw)
      let end_pos = env.calc_offset(rest, base~)
      let comment_text = raw.to_string()
      if env.is_interpolation {
        env.add_lexing_error(start=start_pos, end=end_pos, InterpInvalidComment)
      }
      if env.comment {
        let comment = Comment::{
          content: comment_text,
          kind: InlineTrailing,
          consumed_by_docstring: false,
        }
        preserve_comment(comment, start_pos, end_pos)
        env.add_token_with_loc(COMMENT(comment), start=start_pos, end=end_pos)
      }
      lex_tokens(rest, base~, env~, preserve_comment~)
    }

    // Character literals - basic cases
    ("'" ("[^\\\n\r']" as raw) "'", rest) => {
      let start_pos = env.calc_offset(input, base~)
      env.register_surrogate_pair_for_char(raw)
      let end_pos = env.calc_offset(rest, base~)
      env.add_token_with_loc(CHAR([raw]), start=start_pos, end=end_pos)
      lex_tokens(rest, base~, env~, preserve_comment~)
    }

    // Character literals - escape sequences
    ("'" ("\\[\\'\"ntbrf/ ]" as raw) "'", rest) => {
      env.add_token_with_loc(
        CHAR(raw.to_string()),
        start=env.calc_offset(input, base~),
        end=env.calc_offset(rest, base~),
      )
      lex_tokens(rest, base~, env~, preserve_comment~)
    }

    // Character literals - hex escape
    ("'" ("\\x[0-9a-fA-F]{2}" as raw) "'", rest) => {
      env.add_token_with_loc(
        CHAR(raw.to_string()),
        start=env.calc_offset(input, base~),
        end=env.calc_offset(rest, base~),
      )
      lex_tokens(rest, base~, env~, preserve_comment~)
    }

    // Character literals - octal escape
    ("'" ("\\o[0-3][0-7]{2}" as raw) "'", rest) => {
      env.add_token_with_loc(
        CHAR(raw.to_string()),
        start=env.calc_offset(input, base~),
        end=env.calc_offset(rest, base~),
      )
      lex_tokens(rest, base~, env~, preserve_comment~)
    }

    // Character literals - unicode escape
    ("'" ("\\u[0-9a-fA-F]{4}" as raw) "'", rest) => {
      env.add_token_with_loc(
        CHAR(raw.to_string()),
        start=env.calc_offset(input, base~),
        end=env.calc_offset(rest, base~),
      )
      lex_tokens(rest, base~, env~, preserve_comment~)
    }

    // Character literals - unicode escape with braces
    ("'" ("\\u[{][0-9a-fA-F]+[}]" as raw) "'", rest) => {
      let char_text = raw.to_string()
      env.add_token_with_loc(
        CHAR(char_text),
        start=env.calc_offset(input, base~),
        end=env.calc_offset(rest, base~),
      )
      lex_tokens(rest, base~, env~, preserve_comment~)
    }

    // Regex literals - re"..."
    ("re\"", rest) => {
      let start_pos = env.calc_offset(input, base~)
      let (rest, interps) = lex_string(
        rest,
        base~,
        env~,
        end_with_newline=false,
        allow_interp=true,
        start_pos~,
      )
      let tok : @tokens.Token = match interps {
        [InterpLit(repr~, ..)] => REGEX_LITERAL(repr)
        interps => REGEX_INTERP(interps)
      }
      let end_pos = env.calc_offset(rest, base~)
      env.add_token_with_loc(tok, start=start_pos, end=end_pos)
      lex_tokens(rest, base~, env~, preserve_comment~)
    }

    // String literals - basic double-quoted string
    ("\"", rest) => {
      let start_pos = env.calc_offset(input, base~)
      let (rest, interps) = lex_string(
        rest,
        base~,
        env~,
        end_with_newline=false,
        allow_interp=true,
        start_pos~,
      )
      let tok : @tokens.Token = match interps {
        [InterpLit(repr~, ..)] => STRING(repr)
        interps => INTERP(interps)
      }
      let end_pos = env.calc_offset(rest, base~)
      env.add_token_with_loc(tok, start=start_pos, end=end_pos)
      lex_tokens(rest, base~, env~, preserve_comment~)
    }

    // Byte string literal
    ("b\"", rest) => {
      let start_pos = env.calc_offset(input, base~)
      let (rest, interps) = lex_string(
        rest,
        base~,
        env~,
        end_with_newline=false,
        allow_interp=false,
        start_pos~,
      )
      let tok : @tokens.Token = match interps {
        [InterpLit(repr~, ..)] => BYTES(repr)
        _interps => panic() // byte strings don't support interpolation
      }
      env.add_token_with_loc(
        tok,
        start=start_pos,
        end=env.calc_offset(rest, base~),
      )
      lex_tokens(rest, base~, env~, preserve_comment~)
    }

    // Byte literals - hex escape
    ("b'\\x[0-9a-fA-F]{2}'" as raw, rest) => {
      let literal = try! raw.sub(start=2, end=raw.length() - 1).to_string() // Remove b' and '
      env.add_token_with_loc(
        BYTE(literal),
        start=env.calc_offset(input, base~),
        end=env.calc_offset(rest, base~),
      )
      lex_tokens(rest, base~, env~, preserve_comment~)
    }

    // Byte literals - octal escape
    ("b'\\o[0-3][0-7]{2}'" as raw, rest) => {
      let literal = try! raw.sub(start=2, end=raw.length() - 1).to_string() // Remove b' and '
      env.add_token_with_loc(
        BYTE(literal),
        start=env.calc_offset(input, base~),
        end=env.calc_offset(rest, base~),
      )
      lex_tokens(rest, base~, env~, preserve_comment~)
    }

    // Byte literals - ASCII character
    ("b'[\x00-\x7F]'" as raw, rest) => {
      let ascii_char = try! raw.sub(start=2, end=raw.length() - 1).to_string() // Remove b' and '
      env.add_token_with_loc(
        BYTE(ascii_char),
        start=env.calc_offset(input, base~),
        end=env.calc_offset(rest, base~),
      )
      lex_tokens(rest, base~, env~, preserve_comment~)
    }

    // Byte literals - escape sequences
    ("b'\\[\\'\"`ntbrf/ ]'" as raw, rest) => {
      let literal = try! raw.sub(start=2, end=raw.length() - 1).to_string() // Remove b' and '
      env.add_token_with_loc(
        BYTE(literal),
        start=env.calc_offset(input, base~),
        end=env.calc_offset(rest, base~),
      )
      lex_tokens(rest, base~, env~, preserve_comment~)
    }

    // Invalid byte literal
    ("b'", rest) => {
      let start = env.calc_offset(base~, input)
      let rest = lex_invalid_byte(rest, base~, env~, start~)
      lex_tokens(rest, base~, env~, preserve_comment~)
    }

    // Float literals
    ("[0-9](_|[0-9])*\.[0-9_]*([eE][+\-]?[0-9](_|[0-9])*)?F" as float, rest) => {
      env.add_token_with_loc(
        FLOAT(float.to_string()),
        start=env.calc_offset(input, base~),
        end=env.calc_offset(rest, base~),
      )
      lex_tokens(rest, base~, env~, preserve_comment~)
    }
    (
      "0[xX][0-9a-fA-F](_|[0-9a-fA-F])*\.[0-9a-fA-F_]*([pP][+\-]?[0-9](_|[0-9])*)?F" as float,
      rest
    ) => {
      env.add_token_with_loc(
        FLOAT(float.to_string()),
        start=env.calc_offset(input, base~),
        end=env.calc_offset(rest, base~),
      )
      lex_tokens(rest, base~, env~, preserve_comment~)
    }

    // Double literals
    ("[0-9](_|[0-9])*\.[0-9_]*([eE][+\-]?[0-9](_|[0-9])*)?" as double, rest) => {
      env.add_token_with_loc(
        DOUBLE(double.to_string()),
        start=env.calc_offset(input, base~),
        end=env.calc_offset(rest, base~),
      )
      lex_tokens(rest, base~, env~, preserve_comment~)
    }
    (
      "0[xX][0-9a-fA-F](_|[0-9a-fA-F])*\.[0-9a-fA-F_]*([pP][+\-]?[0-9](_|[0-9])*)?" as double,
      rest
    ) => {
      env.add_token_with_loc(
        DOUBLE(double.to_string()),
        start=env.calc_offset(input, base~),
        end=env.calc_offset(rest, base~),
      )
      lex_tokens(rest, base~, env~, preserve_comment~)
    }

    // Integer literals with range operator handling

    (
      "([0-9](_|[0-9])*|0[xX][0-9a-fA-F](_|[0-9a-fA-F])*|0[Oo][0-7](_|[0-7])*|0[Bb][01](_|[01])*)((UL|U|L|N)?)\.\." as integer_with_range,
      _rest
    ) => {
      // Need to handle integer..range specially
      let integer_end = integer_with_range.length() - 2 // Remove ".."
      let integer = try! integer_with_range
        .sub(start=0, end=integer_end)
        .to_string()
      env.add_token_with_loc(
        INT(integer),
        start=env.calc_offset(input, base~),
        end=env.calc_offset(input, base~) + integer_end,
      )
      // Put back the ".." part
      let dotdot_input = try! input[integer_end:]
      lex_tokens(dotdot_input, base~, env~, preserve_comment~)
    }

    // Regular integer literals
    ("[0-9](_|[0-9])*(UL|U|L|N)?" as integer, rest) => {
      env.add_token_with_loc(
        INT(integer.to_string()),
        start=env.calc_offset(input, base~),
        end=env.calc_offset(rest, base~),
      )
      lex_tokens(rest, base~, env~, preserve_comment~)
    }
    ("0[xX][0-9a-fA-F](_|[0-9a-fA-F])*(UL|U|L|N)?" as integer, rest) => {
      env.add_token_with_loc(
        INT(integer.to_string()),
        start=env.calc_offset(input, base~),
        end=env.calc_offset(rest, base~),
      )
      lex_tokens(rest, base~, env~, preserve_comment~)
    }
    ("0[Oo][0-7](_|[0-7])*(UL|U|L|N)?" as integer, rest) => {
      env.add_token_with_loc(
        INT(integer.to_string()),
        start=env.calc_offset(input, base~),
        end=env.calc_offset(rest, base~),
      )
      lex_tokens(rest, base~, env~, preserve_comment~)
    }
    ("0[Bb][01](_|[01])*(UL|U|L|N)?" as integer, rest) => {
      env.add_token_with_loc(
        INT(integer.to_string()),
        start=env.calc_offset(input, base~),
        end=env.calc_offset(rest, base~),
      )
      lex_tokens(rest, base~, env~, preserve_comment~)
    }

    // Upper case identifiers (types) - simplified pattern for ASCII and basic Unicode

    (
      "[A-Z][\u{30}-\u{39}\u{41}-\u{5a}\u{5f}-\u{5f}\u{61}-\u{7a}\u{a1}-\u{ac}\u{ae}-\u{2af}\u{1100}-\u{11ff}\u{1e00}-\u{1eff}\u{2070}-\u{209f}\u{2150}-\u{218f}\u{2e80}-\u{2eff}\u{2ff0}-\u{2fff}\u{3001}-\u{30ff}\u{31c0}-\u{9fff}\u{ac00}-\u{d7ff}\u{f900}-\u{faff}\u{fe00}-\u{fe0f}\u{fe30}-\u{fe4f}\u{1f000}-\u{1fbff}\u{20000}-\u{2a6df}\u{2a700}-\u{2ebef}\u{2f800}-\u{2fa1f}\u{30000}-\u{323af}\u{e0100}-\u{e01ef}]*" as raw,
      rest
    ) => {
      let start_pos = env.calc_offset(input, base~)
      env.register_surrogate_pair(raw)
      let end_pos = env.calc_offset(rest, base~)
      env.add_token_with_loc(
        @tokens.Token::UIDENT(raw.to_string()),
        start=start_pos,
        end=end_pos,
      )
      lex_tokens(rest, base~, env~, preserve_comment~)
    }

    // Post label identifiers (name~) - simplified pattern
    ("[a-z_][a-zA-Z0-9_]*~" as raw, rest) => {
      let ident = try! raw.sub(start=0, end=raw.length() - 1).to_string() // Remove ~
      env.add_token_with_loc(
        @tokens.Token::POST_LABEL(ident),
        start=env.calc_offset(input, base~),
        end=env.calc_offset(rest, base~),
      )
      lex_tokens(rest, base~, env~, preserve_comment~)
    }

    // Lower case identifiers and keywords

    (
      "[\u{5f}-\u{5f}\u{61}-\u{7a}\u{a1}-\u{ac}\u{ae}-\u{2af}\u{1100}-\u{11ff}\u{1e00}-\u{1eff}\u{2070}-\u{209f}\u{2150}-\u{218f}\u{2e80}-\u{2eff}\u{2ff0}-\u{2fff}\u{3001}-\u{30ff}\u{31c0}-\u{9fff}\u{ac00}-\u{d7ff}\u{f900}-\u{faff}\u{fe00}-\u{fe0f}\u{fe30}-\u{fe4f}\u{1f000}-\u{1fbff}\u{20000}-\u{2a6df}\u{2a700}-\u{2ebef}\u{2f800}-\u{2fa1f}\u{30000}-\u{323af}\u{e0100}-\u{e01ef}][\u{30}-\u{39}\u{41}-\u{5a}\u{5f}-\u{5f}\u{61}-\u{7a}\u{a1}-\u{ac}\u{ae}-\u{2af}\u{1100}-\u{11ff}\u{1e00}-\u{1eff}\u{2070}-\u{209f}\u{2150}-\u{218f}\u{2e80}-\u{2eff}\u{2ff0}-\u{2fff}\u{3001}-\u{30ff}\u{31c0}-\u{9fff}\u{ac00}-\u{d7ff}\u{f900}-\u{faff}\u{fe00}-\u{fe0f}\u{fe30}-\u{fe4f}\u{1f000}-\u{1fbff}\u{20000}-\u{2a6df}\u{2a700}-\u{2ebef}\u{2f800}-\u{2fa1f}\u{30000}-\u{323af}\u{e0100}-\u{e01ef}]*" as raw,
      rest
    ) => {
      let raw_str = raw.to_string()
      let start_pos = env.calc_offset(input, base~)
      env.register_surrogate_pair(raw)
      let end_pos = env.calc_offset(rest, base~)
      if reserved_keyword_table.contains(raw_str) {
        env.add_lexing_error(
          Reserved_keyword(raw_str),
          start=start_pos,
          end=end_pos,
        )
      }
      let token = match keyword_table.get(raw_str) {
        None => @tokens.Token::LIDENT(raw_str)
        Some(tok) => tok
      }
      env.add_token_with_loc(token, start=start_pos, end=end_pos)
      lex_tokens(rest, base~, env~, preserve_comment~)
    }

    // Operators and punctuation - order matters for longer operators first

    // Augmented assignment operators
    ("[+\-*/%]=" as op, rest) => {
      let op_char = try! op.sub(start=0, end=1).to_string()
      env.add_token_with_loc(
        @tokens.Token::AUGMENTED_ASSIGNMENT(op_char),
        start=env.calc_offset(input, base~),
        end=env.calc_offset(rest, base~),
      )
      lex_tokens(rest, base~, env~, preserve_comment~)
    }

    // Multiple character operators
    ("&&", rest) => {
      env.add_token_with_loc(
        @tokens.Token::AMPERAMPER,
        start=env.calc_offset(input, base~),
        end=env.calc_offset(rest, base~),
      )
      lex_tokens(rest, base~, env~, preserve_comment~)
    }
    ("\|\|", rest) => {
      env.add_token_with_loc(
        @tokens.Token::BARBAR,
        start=env.calc_offset(input, base~),
        end=env.calc_offset(rest, base~),
      )
      lex_tokens(rest, base~, env~, preserve_comment~)
    }
    ("\|>", rest) => {
      env.add_token_with_loc(
        @tokens.Token::PIPE,
        start=env.calc_offset(input, base~),
        end=env.calc_offset(rest, base~),
      )
      lex_tokens(rest, base~, env~, preserve_comment~)
    }
    ("try\?", rest) => {
      env.add_token_with_loc(
        @tokens.Token::TRY_QUESTION,
        start=env.calc_offset(input, base~),
        end=env.calc_offset(rest, base~),
      )
      lex_tokens(rest, base~, env~, preserve_comment~)
    }
    ("try!", rest) => {
      env.add_token_with_loc(
        @tokens.Token::TRY_EXCLAMATION,
        start=env.calc_offset(input, base~),
        end=env.calc_offset(rest, base~),
      )
      lex_tokens(rest, base~, env~, preserve_comment~)
    }
    ("lexmatch\?", rest) => {
      env.add_token_with_loc(
        @tokens.Token::LEXMATCH_QUESTION,
        start=env.calc_offset(input, base~),
        end=env.calc_offset(rest, base~),
      )
      lex_tokens(rest, base~, env~, preserve_comment~)
    }
    ("==", rest) => {
      env.add_token_with_loc(
        @tokens.Token::INFIX1("=="),
        start=env.calc_offset(input, base~),
        end=env.calc_offset(rest, base~),
      )
      lex_tokens(rest, base~, env~, preserve_comment~)
    }
    ("!=", rest) => {
      env.add_token_with_loc(
        @tokens.Token::INFIX1("!="),
        start=env.calc_offset(input, base~),
        end=env.calc_offset(rest, base~),
      )
      lex_tokens(rest, base~, env~, preserve_comment~)
    }
    ("<=", rest) => {
      env.add_token_with_loc(
        @tokens.Token::INFIX1("<="),
        start=env.calc_offset(input, base~),
        end=env.calc_offset(rest, base~),
      )
      lex_tokens(rest, base~, env~, preserve_comment~)
    }
    (">\.\.", rest) => {
      env.add_token_with_loc(
        @tokens.Token::RANGE_EXCLUSIVE_REV,
        start=env.calc_offset(input, base~),
        end=env.calc_offset(rest, base~),
      )
      lex_tokens(rest, base~, env~, preserve_comment~)
    }
    (">=\.\.", rest) => {
      env.add_token_with_loc(
        @tokens.Token::RANGE_INCLUSIVE_REV,
        start=env.calc_offset(input, base~),
        end=env.calc_offset(rest, base~),
      )
      lex_tokens(rest, base~, env~, preserve_comment~)
    }
    (">=", rest) => {
      env.add_token_with_loc(
        @tokens.Token::INFIX1(">="),
        start=env.calc_offset(input, base~),
        end=env.calc_offset(rest, base~),
      )
      lex_tokens(rest, base~, env~, preserve_comment~)
    }
    ("<<", rest) => {
      env.add_token_with_loc(
        @tokens.Token::INFIX2("<<"),
        start=env.calc_offset(input, base~),
        end=env.calc_offset(rest, base~),
      )
      lex_tokens(rest, base~, env~, preserve_comment~)
    }
    (">>", rest) => {
      env.add_token_with_loc(
        @tokens.Token::INFIX2(">>"),
        start=env.calc_offset(input, base~),
        end=env.calc_offset(rest, base~),
      )
      lex_tokens(rest, base~, env~, preserve_comment~)
    }
    ("::", rest) => {
      env.add_token_with_loc(
        @tokens.Token::COLONCOLON,
        start=env.calc_offset(input, base~),
        end=env.calc_offset(rest, base~),
      )
      lex_tokens(rest, base~, env~, preserve_comment~)
    }

    // Dots and ranges
    ("\.\.\.", rest) => {
      env.add_token_with_loc(
        @tokens.Token::ELLIPSIS,
        start=env.calc_offset(input, base~),
        end=env.calc_offset(rest, base~),
      )
      lex_tokens(rest, base~, env~, preserve_comment~)
    }
    ("\.\.<=", rest) => {
      env.add_token_with_loc(
        @tokens.Token::RANGE_LT_INCLUSIVE,
        start=env.calc_offset(input, base~),
        end=env.calc_offset(rest, base~),
      )
      lex_tokens(rest, base~, env~, preserve_comment~)
    }
    ("\.\.=", rest) => {
      env.add_token_with_loc(
        @tokens.Token::RANGE_INCLUSIVE,
        start=env.calc_offset(input, base~),
        end=env.calc_offset(rest, base~),
      )
      lex_tokens(rest, base~, env~, preserve_comment~)
    }
    ("\.\.<", rest) => {
      env.add_token_with_loc(
        @tokens.Token::RANGE_EXCLUSIVE,
        start=env.calc_offset(input, base~),
        end=env.calc_offset(rest, base~),
      )
      lex_tokens(rest, base~, env~, preserve_comment~)
    }
    ("\.\.", rest) => {
      env.add_token_with_loc(
        @tokens.Token::DOTDOT,
        start=env.calc_offset(input, base~),
        end=env.calc_offset(rest, base~),
      )
      lex_tokens(rest, base~, env~, preserve_comment~)
    }

    // Dot with identifier

    (
      "\."
      (
        "[A-Z][\u{30}-\u{39}\u{41}-\u{5a}\u{5f}-\u{5f}\u{61}-\u{7a}\u{a1}-\u{ac}\u{ae}-\u{2af}\u{1100}-\u{11ff}\u{1e00}-\u{1eff}\u{2070}-\u{209f}\u{2150}-\u{218f}\u{2e80}-\u{2eff}\u{2ff0}-\u{2fff}\u{3001}-\u{30ff}\u{31c0}-\u{9fff}\u{ac00}-\u{d7ff}\u{f900}-\u{faff}\u{fe00}-\u{fe0f}\u{fe30}-\u{fe4f}\u{1f000}-\u{1fbff}\u{20000}-\u{2a6df}\u{2a700}-\u{2ebef}\u{2f800}-\u{2fa1f}\u{30000}-\u{323af}\u{e0100}-\u{e01ef}]*" as ident
      ),
      rest
    ) => {
      let name = ident.to_string()
      let start_pos = env.calc_offset(input, base~) + 1
      env.register_surrogate_pair(ident)
      let end_pos = env.calc_offset(rest, base~)
      env.add_token_with_loc(
        @tokens.Token::DOT_UIDENT(name),
        start=start_pos,
        end=end_pos,
      )
      lex_tokens(rest, base~, env~, preserve_comment~)
    }
    (
      "\."
      (
        "([\u{5f}-\u{5f}\u{61}-\u{7a}\u{a1}-\u{ac}\u{ae}-\u{2af}\u{1100}-\u{11ff}\u{1e00}-\u{1eff}\u{2070}-\u{209f}\u{2150}-\u{218f}\u{2e80}-\u{2eff}\u{2ff0}-\u{2fff}\u{3001}-\u{30ff}\u{31c0}-\u{9fff}\u{ac00}-\u{d7ff}\u{f900}-\u{faff}\u{fe00}-\u{fe0f}\u{fe30}-\u{fe4f}\u{1f000}-\u{1fbff}\u{20000}-\u{2a6df}\u{2a700}-\u{2ebef}\u{2f800}-\u{2fa1f}\u{30000}-\u{323af}\u{e0100}-\u{e01ef}][\u{30}-\u{39}\u{41}-\u{5a}\u{5f}-\u{5f}\u{61}-\u{7a}\u{a1}-\u{ac}\u{ae}-\u{2af}\u{1100}-\u{11ff}\u{1e00}-\u{1eff}\u{2070}-\u{209f}\u{2150}-\u{218f}\u{2e80}-\u{2eff}\u{2ff0}-\u{2fff}\u{3001}-\u{30ff}\u{31c0}-\u{9fff}\u{ac00}-\u{d7ff}\u{f900}-\u{faff}\u{fe00}-\u{fe0f}\u{fe30}-\u{fe4f}\u{1f000}-\u{1fbff}\u{20000}-\u{2a6df}\u{2a700}-\u{2ebef}\u{2f800}-\u{2fa1f}\u{30000}-\u{323af}\u{e0100}-\u{e01ef}]*)?" as ident
      ),
      rest
    ) => {
      let name = ident.to_string()
      let start_pos = env.calc_offset(input, base~) + 1
      env.register_surrogate_pair(ident)
      let end_pos = env.calc_offset(rest, base~)
      env.add_token_with_loc(
        @tokens.Token::DOT_LIDENT(name),
        start=start_pos,
        end=end_pos,
      )
      lex_tokens(rest, base~, env~, preserve_comment~)
    }

    // Dot with number
    ("\.[0-9]+" as dot_int, rest) => {
      let digits_str = try! dot_int.sub(start=1, end=dot_int.length()) // Remove the '.'
      let idx = @strconv.parse_int(digits_str) catch {
        _ => {
          let full_str = dot_int.to_string()
          env.add_lexing_error(
            InvalidDotInt(full_str),
            start=env.calc_offset(input, base~),
            end=env.calc_offset(rest, base~),
          )
          0
        }
      }
      env.add_token_with_loc(
        @tokens.Token::DOT_INT(idx),
        start=env.calc_offset(input, base~) + 1,
        end=env.calc_offset(rest, base~),
      )
      lex_tokens(rest, base~, env~, preserve_comment~)
    }

    // Dot with parenthesis
    ("\.\(", rest) => {
      env.add_token_with_loc(
        @tokens.Token::DOT_LPAREN,
        start=env.calc_offset(input, base~),
        end=env.calc_offset(rest, base~),
      )
      lex_tokens(rest, base~, env~, preserve_comment~)
    }

    // Single character operators and punctuation
    ("&", rest) => {
      env.add_token_with_loc(
        @tokens.Token::AMPER,
        start=env.calc_offset(input, base~),
        end=env.calc_offset(rest, base~),
      )
      lex_tokens(rest, base~, env~, preserve_comment~)
    }
    ("\|", rest) => {
      env.add_token_with_loc(
        @tokens.Token::BAR,
        start=env.calc_offset(input, base~),
        end=env.calc_offset(rest, base~),
      )
      lex_tokens(rest, base~, env~, preserve_comment~)
    }
    ("\^", rest) => {
      env.add_token_with_loc(
        @tokens.Token::CARET,
        start=env.calc_offset(input, base~),
        end=env.calc_offset(rest, base~),
      )
      lex_tokens(rest, base~, env~, preserve_comment~)
    }
    ("\(", rest) => {
      env.add_token_with_loc(
        @tokens.Token::LPAREN,
        start=env.calc_offset(input, base~),
        end=env.calc_offset(rest, base~),
      )
      lex_tokens(rest, base~, env~, preserve_comment~)
    }
    ("\)", rest) => {
      env.add_token_with_loc(
        @tokens.Token::RPAREN,
        start=env.calc_offset(input, base~),
        end=env.calc_offset(rest, base~),
      )
      lex_tokens(rest, base~, env~, preserve_comment~)
    }
    ("\*", rest) => {
      env.add_token_with_loc(
        @tokens.Token::INFIX3("*"),
        start=env.calc_offset(input, base~),
        end=env.calc_offset(rest, base~),
      )
      lex_tokens(rest, base~, env~, preserve_comment~)
    }
    ("/", rest) => {
      env.add_token_with_loc(
        @tokens.Token::INFIX3("/"),
        start=env.calc_offset(input, base~),
        end=env.calc_offset(rest, base~),
      )
      lex_tokens(rest, base~, env~, preserve_comment~)
    }
    ("%", rest) => {
      env.add_token_with_loc(
        @tokens.Token::INFIX3("%"),
        start=env.calc_offset(input, base~),
        end=env.calc_offset(rest, base~),
      )
      lex_tokens(rest, base~, env~, preserve_comment~)
    }
    (",", rest) => {
      env.add_token_with_loc(
        @tokens.Token::COMMA,
        start=env.calc_offset(input, base~),
        end=env.calc_offset(rest, base~),
      )
      lex_tokens(rest, base~, env~, preserve_comment~)
    }
    (":", rest) => {
      env.add_token_with_loc(
        @tokens.Token::COLON,
        start=env.calc_offset(input, base~),
        end=env.calc_offset(rest, base~),
      )
      lex_tokens(rest, base~, env~, preserve_comment~)
    }
    (";", rest) => {
      env.add_token_with_loc(
        @tokens.Token::SEMI(true),
        start=env.calc_offset(input, base~),
        end=env.calc_offset(rest, base~),
      )
      lex_tokens(rest, base~, env~, preserve_comment~)
    }
    ("=", rest) => {
      env.add_token_with_loc(
        @tokens.Token::EQUAL,
        start=env.calc_offset(input, base~),
        end=env.calc_offset(rest, base~),
      )
      lex_tokens(rest, base~, env~, preserve_comment~)
    }
    ("<", rest) => {
      env.add_token_with_loc(
        @tokens.Token::INFIX1("<"),
        start=env.calc_offset(input, base~),
        end=env.calc_offset(rest, base~),
      )
      lex_tokens(rest, base~, env~, preserve_comment~)
    }
    (">", rest) => {
      env.add_token_with_loc(
        @tokens.Token::INFIX1(">"),
        start=env.calc_offset(input, base~),
        end=env.calc_offset(rest, base~),
      )
      lex_tokens(rest, base~, env~, preserve_comment~)
    }
    ("\[", rest) => {
      env.add_token_with_loc(
        @tokens.Token::LBRACKET,
        start=env.calc_offset(input, base~),
        end=env.calc_offset(rest, base~),
      )
      lex_tokens(rest, base~, env~, preserve_comment~)
    }
    ("\]", rest) => {
      env.add_token_with_loc(
        @tokens.Token::RBRACKET,
        start=env.calc_offset(input, base~),
        end=env.calc_offset(rest, base~),
      )
      lex_tokens(rest, base~, env~, preserve_comment~)
    }
    ("[{]", rest) => {
      env.add_token_with_loc(
        @tokens.Token::LBRACE,
        start=env.calc_offset(input, base~),
        end=env.calc_offset(rest, base~),
      )
      lex_tokens(rest, base~, env~, preserve_comment~)
    }
    ("[}]", rest) => {
      env.add_token_with_loc(
        @tokens.Token::RBRACE,
        start=env.calc_offset(input, base~),
        end=env.calc_offset(rest, base~),
      )
      lex_tokens(rest, base~, env~, preserve_comment~)
    }
    ("\+", rest) => {
      env.add_token_with_loc(
        @tokens.Token::PLUS,
        start=env.calc_offset(input, base~),
        end=env.calc_offset(rest, base~),
      )
      lex_tokens(rest, base~, env~, preserve_comment~)
    }
    ("-", rest) => {
      env.add_token_with_loc(
        @tokens.Token::MINUS,
        start=env.calc_offset(input, base~),
        end=env.calc_offset(rest, base~),
      )
      lex_tokens(rest, base~, env~, preserve_comment~)
    }
    ("\?", rest) => {
      env.add_token_with_loc(
        @tokens.Token::QUESTION,
        start=env.calc_offset(input, base~),
        end=env.calc_offset(rest, base~),
      )
      lex_tokens(rest, base~, env~, preserve_comment~)
    }
    ("!", rest) => {
      env.add_token_with_loc(
        @tokens.Token::EXCLAMATION,
        start=env.calc_offset(input, base~),
        end=env.calc_offset(rest, base~),
      )
      lex_tokens(rest, base~, env~, preserve_comment~)
    }

    // Attributes

    // #ident.dot_ident with payload

    (
      "#"
      ("[a-zA-Z_][a-zA-Z0-9_]*" as ident)
      "\."
      ("[a-zA-Z_][a-zA-Z0-9_]*" as dot_ident)
      ("[^\r\n]*" as raw_payload),
      rest
    ) => {
      if env.is_interpolation {
        env.add_lexing_error(
          start=env.calc_offset(input, base~),
          end=env.calc_offset(rest, base~),
          InterpInvalidAttribute,
        )
      } else {
        let ident = ident.to_string()
        let dot_ident = dot_ident.to_string()
        let raw_payload = raw_payload.to_string()
        env.add_token_with_loc(
          @tokens.Token::ATTRIBUTE((ident, Some(dot_ident), raw_payload)),
          start=env.calc_offset(input, base~),
          end=env.calc_offset(rest, base~),
        )
      }
      lex_tokens(rest, base~, env~, preserve_comment~)
    }

    // #ident with payload

    ("#" ("[a-zA-Z_][a-zA-Z0-9_]*" as ident) ("[^\r\n]*" as raw_payload), rest) => {
      if env.is_interpolation {
        env.add_lexing_error(
          start=env.calc_offset(input, base~),
          end=env.calc_offset(rest, base~),
          InterpInvalidAttribute,
        )
      } else {
        let ident = ident.to_string()
        let raw_payload = raw_payload.to_string()
        env.add_token_with_loc(
          @tokens.Token::ATTRIBUTE((ident, None, raw_payload)),
          start=env.calc_offset(input, base~),
          end=env.calc_offset(rest, base~),
        )
      }
      lex_tokens(rest, base~, env~, preserve_comment~)
    }

    // Multiline string interpolation $|
    ("\$\|", rest) => {
      if env.is_interpolation {
        env.add_lexing_error(
          start=env.calc_offset(input, base~),
          end=env.calc_offset(rest, base~),
          InterpInvalidMultilineString,
        )
      }
      let start_pos = env.calc_offset(input, base~)
      let (rest, interps) = lex_string(
        rest,
        base~,
        env~,
        end_with_newline=true,
        allow_interp=true,
        start_pos~,
      )
      let tok = @tokens.Token::MULTILINE_INTERP(interps)
      env.add_token_with_loc(
        tok,
        start=start_pos,
        end=env.calc_offset(rest, base~),
      )
      lex_tokens(rest, base~, env~, preserve_comment~)
    }

    // Multiline string #|
    ("#\|[^\r\n]*" as multiline_str, rest) => {
      let start_pos = env.calc_offset(input, base~)
      env.register_surrogate_pair(multiline_str)
      let end_pos = env.calc_offset(rest, base~)
      if env.is_interpolation {
        env.add_lexing_error(
          start=start_pos,
          end=end_pos,
          InterpInvalidMultilineString,
        )
      }
      let content_str = multiline_str.to_string()
      let content = try! content_str.sub(start=2, end=content_str.length()) // Remove #|
      env.add_token_with_loc(
        @tokens.Token::MULTILINE_STRING(content.to_string()),
        start=start_pos,
        end=end_pos,
      )
      lex_tokens(rest, base~, env~, preserve_comment~)
    }

    // Package name @package/name - simplified pattern
    ("@" ("[a-zA-Z_][a-zA-Z0-9_/]*" as pkg_name_without_at), rest) => {
      env.add_token_with_loc(
        @tokens.Token::PACKAGE_NAME(pkg_name_without_at.to_string()),
        start=env.calc_offset(input, base~),
        end=env.calc_offset(rest, base~),
      )
      lex_tokens(rest, base~, env~, preserve_comment~)
    }

    // EOF case
    ("$", _) => {
      let end_pos = env.calc_offset(input, base~)
      env.add_token_with_loc(EOF, start=end_pos, end=end_pos)
    }

    // Error case - any remaining character
    ("." as c, rest) => {
      env.add_lexing_error(
        IllegalCharacter(c),
        start=env.calc_offset(input, base~),
        end=env.calc_offset(input, base~) + 1,
      )
      lex_tokens(rest, base~, env~, preserve_comment~)
    }
    _ => panic()
  }
}

///|
fn lex_invalid_byte(
  input : StringView,
  base~ : StringView,
  env~ : LexEnv,
  start~ : Int,
) -> StringView {
  let invalid_byte_repr_buf = StringBuilder::new()
  fn process_invalid_byte(input : StringView) -> StringView {
    lexmatch input with longest {
      ("'", rest) => {
        env.add_lexing_error(
          InvalidByteLiteral(invalid_byte_repr_buf.to_string()),
          start~,
          end=env.calc_offset(base~, rest),
        )
        rest
      }
      ("\r|\n", _) => {
        env.add_lexing_error(
          start=env.calc_offset(base~, input),
          end=env.calc_offset(base~, input),
          UnterminatedStringInVariableInterploation,
        )
        input
      }
      "" => {
        env.add_lexing_error(
          InvalidByteLiteral(invalid_byte_repr_buf.to_string()),
          start~,
          end=env.calc_offset(base~, input),
        )
        input
      }
      ("." as c, rest) => {
        env.register_surrogate_pair_for_char(c)
        invalid_byte_repr_buf.write_char(c)
        process_invalid_byte(rest)
      }
      _ => panic()
    }
  }

  process_invalid_byte(input)
}

///|
fn lex_string(
  input : StringView,
  base~ : StringView,
  env~ : LexEnv,
  end_with_newline~ : Bool,
  allow_interp~ : Bool,
  start_pos~ : Int,
) -> (StringView, Array[InterpElem]) {
  let string_repr_buf = StringBuilder::new()
  let interps = []
  fn add_literal(repr : String, start : Int, end : Int) -> Unit {
    interps.push(
      @tokens.InterpElem::InterpLit(repr~, loc=Location::{
        start: env.make_pos(start),
        end: env.make_pos(end),
      }),
    )
  }

  fn process_interpolation(input : StringView) -> (StringView, Int) {
    lexmatch input with longest {
      (
        "[\u0009\u000B\u000C\u0020\u00A0\uFEFF\u1680\u2000-\u200A\u202F\u205F\u3000]*\}",
        // spaces + '}'
        rest
      ) => (rest, env.calc_offset(base~, input))
      "" => {
        env.add_lexing_error(
          start=env.calc_offset(base~, input),
          end=env.calc_offset(base~, input),
          UnterminatedString,
        )
        (input, env.calc_offset(base~, input))
      }
      ("\r|\n", _) => {
        env.add_lexing_error(
          start=env.calc_offset(base~, input),
          end=env.calc_offset(base~, input),
          UnterminatedStringInVariableInterploation,
        )
        (input, env.calc_offset(base~, input))
      }
      ("[^\n{}]" as c, rest) => {
        env.register_surrogate_pair_for_char(c)
        string_repr_buf.write_char(c)
        process_interpolation(rest)
      }
      _ => panic()
    }
  }

  fn process_string(input : StringView, start_pos : Int) -> StringView {
    lexmatch input with longest {
      (
        "\"",
        // End of string
        rest
      ) =>
        if end_with_newline {
          string_repr_buf.write_char('"')
          process_string(rest, start_pos)
        } else {
          if not(string_repr_buf.is_empty()) {
            add_literal(
              string_repr_buf.to_string(),
              start_pos,
              env.calc_offset(base~, rest),
            )
          }
          rest
        }

      // Escape sequences
      ("\\[\\'\"`ntbrf/ ]" as raw, rest) => {
        string_repr_buf.write_string(raw.to_string())
        process_string(rest, start_pos)
      }

      // Hex escape
      ("\\x[0-9a-fA-F]{2}" as raw, rest) => {
        string_repr_buf.write_string(raw.to_string())
        process_string(rest, start_pos)
      }

      // Octal escape
      ("\\o[0-3][0-7]{2}" as raw, rest) => {
        string_repr_buf.write_string(raw.to_string())
        process_string(rest, start_pos)
      }

      // Unicode escape
      ("\\u[0-9a-fA-F]{4}" as raw, rest) => {
        string_repr_buf.write_string(raw.to_string())
        process_string(rest, start_pos)
      }

      // Unicode escape with braces
      ("\\u[{][0-9a-fA-F]+[}]" as raw, rest) => {
        string_repr_buf.write_string(raw.to_string())
        process_string(rest, start_pos)
      }

      // String interpolation

      (
        "\\[{][\u0009\u000B\u000C\u0020\u00A0\uFEFF\u1680\u2000-\u200A\u202F\u205F\u3000]*" as raw,
        rest
      ) =>
        if allow_interp {
          if not(string_repr_buf.is_empty()) {
            add_literal(
              string_repr_buf.to_string(),
              start_pos,
              env.calc_offset(base~, input),
            )
          }
          string_repr_buf.reset()
          let interp_start_pos = env.make_pos(env.calc_offset(base~, rest))
          let (rest, interp_end) = process_interpolation(rest)
          let loc = Location::{
            start: interp_start_pos,
            end: env.make_pos(interp_end),
          }
          if string_repr_buf.is_empty() {
            env.add_lexing_error(
              start=env.calc_offset(base~, input),
              end=env.calc_offset(base~, rest),
              InterpMissingExpression,
            )
          } else {
            let source = string_repr_buf.to_string()
            interps.push(
              @tokens.InterpElem::InterpSource(InterpSource::{ source, loc }),
            )
          }
          string_repr_buf.reset()
          process_string(rest, env.calc_offset(base~, rest))
        } else {
          string_repr_buf.write_string(raw.to_string())
          process_string(rest, env.calc_offset(base~, rest))
        }

      // EOF
      "" => {
        env.add_lexing_error(
          start=start_pos,
          end=env.calc_offset(base~, input),
          UnterminatedString,
        )
        if not(string_repr_buf.is_empty()) {
          add_literal(
            string_repr_buf.to_string(),
            start_pos,
            env.calc_offset(base~, input),
          )
        }
        input
      }

      // CRLF
      ("[\r\n]", rest) => {
        let end_pos = env.calc_offset(base~, rest)
        if not(end_with_newline) {
          env.add_lexing_error(start=start_pos, end=end_pos, UnterminatedString)
        }
        if not(string_repr_buf.is_empty()) {
          add_literal(string_repr_buf.to_string(), start_pos, end_pos)
        }
        // Need to back off to handle NEWLINE token
        input
      }

      // Any other character
      ("." as c, rest) => {
        env.register_surrogate_pair_for_char(c)
        string_repr_buf.write_char(c)
        process_string(rest, start_pos)
      }
      _ => panic()
    }
  }

  let rest = process_string(input, start_pos)
  if interps.length() == 0 {
    let interps : Array[InterpElem] = [
      @tokens.InterpElem::InterpLit(repr="", loc=Location::{
        start: env.make_pos(start_pos),
        end: env.make_pos(env.calc_offset(base~, rest)),
      }),
    ]
    (rest, interps)
  } else {
    (rest, interps)
  }
}
