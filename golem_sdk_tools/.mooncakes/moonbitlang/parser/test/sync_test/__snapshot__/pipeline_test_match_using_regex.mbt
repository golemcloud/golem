//!build: -w -28

///|
fn simple(input : BytesView) -> Unit {
  lexmatch input with longest {
    ("((a|b)(cd)?)+$", _) => println(true)
    _ => println(false)
  }
}

///|
fn a_star_capture(input : BytesView) -> Unit {
  lexmatch input with longest {
    ("a*" ("a+" as cap) "a", _) => println(cap)
    _ => println("no match")
  }
}

///|
fn longest_match(input : BytesView) -> Unit {
  println("longest_match over \{input}")
  lexmatch input with longest {
    ("let", _) => println("let")
    ("[A-Za-z_][A-Za-z0-9_]*" as id, _) => println(id)
    _ => println("no match")
  }
}

///|
fn longest_match2(input : BytesView) -> Unit {
  println("longest_match2 over \{input}")
  lexmatch input with longest {
    ("ab$", _) => println(1)
    ("a", _) => println(2)
    _ => println(3)
  }
}

///|
fn test_character_classes() -> Unit raise {
  // Test digit character class
  lexmatch b"123" with longest {
    ("[0-9]+" as digits, _) =>
      if !(digits is "123") {
        fail("test_character_classes")
      }
    ...
  }

  // Test identifier pattern
  lexmatch b"_variable123" with longest {
    ("[a-zA-Z_][a-zA-Z0-9_]*" as id, _) =>
      if !(id is "_variable123") {
        fail("test_character_classes")
      }
    ...
  }

  // Test negated character class
  lexmatch b"xyz" with longest {
    ("[^abc]+" as non_abc, _) =>
      if !(non_abc is "xyz") {
        fail("test_character_classes")
      }
    ...
  }

  // Test character class that should not match
  lexmatch b"abc" with longest {
    ("[^abc]+", _) => if !true { fail("test_character_classes") }
    _ => ()
  }
}

///|
fn test_anchors() -> Unit raise {
  // Test end anchor
  lexmatch b"hello_end" with longest {
    "(.*)end" => ()
    _ => fail("test_anchors")
  }

  // Test pattern that doesn't end with required suffix
  lexmatch b"end_hello" with longest {
    "(.*)end" => fail("test_anchors")
    _ => ()
  }

  // Test start anchor
  fn f(input : BytesView) -> Unit raise {
    lexmatch input with longest {
      "([ ]+|^he)llo" => ()
      _ => fail("test_anchors")
    }
  }

  fn f2(input : BytesView) -> Unit raise {
    lexmatch input with longest {
      "a^" => fail("test_anchors")
      _ => ()
    }
  }

  f(b"hello")
  f(b"   llo")
  lexmatch "a" with longest {
    "a^" => fail("test_anchors")
    _ => ()
  }
}

///|
fn test_nested_groups() -> Unit raise {
  // Test nested captured groups
  lexmatch b"aaabbbccc" with longest {
    (("(a+)(b+)" as outer) ("c+" as rest), _) => {
      if !(outer is "aaabbb") {
        fail("test_nested_groups")
      }
      if !(rest is "ccc") {
        fail("test_nested_groups")
      }
    }
    _ => fail("test_nested_groups")
  }
}

///|
fn test_alternation() -> Unit raise {
  // Test simple alternation
  lexmatch b"cat" with longest {
    ("cat|dog" as animal, _) =>
      if !(animal is "cat") {
        fail("test_alternation")
      }
    ...
  }
  lexmatch b"dog" with longest {
    ("cat|dog" as animal, _) =>
      if !(animal is "dog") {
        fail("test_alternation")
      }
    ...
  }

  // Test alternation with different lengths (longest wins)
  lexmatch b"abc" with longest {
    ("a|ab|abc" as match_result, _) =>
      if !(match_result is "abc") {
        fail("test_alternation")
      }
    ...
  }
}

///|
fn test_quantifiers() -> Unit raise {
  // Test optional, zero-or-more, one-or-more
  lexmatch b"abbbccc" with longest {
    (("a?" as optional) ("b*" as zero_or_more) ("c+" as one_or_more), _) => {
      if !(optional is "a") {
        fail("test_quantifiers")
      }
      if !(zero_or_more is "bbb") {
        fail("test_quantifiers")
      }
      if !(one_or_more is "ccc") {
        fail("test_quantifiers")
      }
    }
    _ => fail("test_quantifiers")
  }

  // Test zero-or-more with empty match
  lexmatch b"ccc" with longest {
    (("a*" as zero_match) ("c+" as one_or_more), _) => {
      if !(zero_match is "") {
        fail("test_quantifiers")
      }
      if !(one_or_more is "ccc") {
        fail("test_quantifiers")
      }
    }
    _ => fail("test_quantifiers")
  }
}

///|
fn test_wildcard_patterns() -> Unit raise {
  // Test dot matches any character
  lexmatch b"axb" with longest {
    ("a.b" as pattern, _) =>
      if !(pattern is "axb") {
        fail("test_wildcard_patterns")
      }
    ...
  }

  // Test .* matches everything
  lexmatch b"hello world 123" with longest {
    (".*" as everything, _) =>
      if !(everything is "hello world 123") {
        fail("test_wildcard_patterns")
      }
    ...
  }

  // Test .+ matches one or more
  lexmatch b"x" with longest {
    (".+" as one_or_more, _) =>
      if !(one_or_more is "x") {
        fail("test_wildcard_patterns")
      }
    ...
  }
}

///|
fn test_escaped_characters() -> Unit raise {
  // Test escaped dot
  lexmatch b"3.14" with longest {
    ("[0-9]+\.[0-9]+" as number, _) =>
      if !(number is "3.14") {
        fail("test_escaped_characters")
      }
    ...
  }

  // Test escaped tab
  lexmatch b"hello\tworld" with longest {
    ("[a-zA-Z]+\t[a-zA-Z]+" as tab_separated, _) =>
      if !(tab_separated is "hello\tworld") {
        fail("test_escaped_characters")
      }
    ...
  }
  lexmatch b"{}" with longest {
    ("[{][}]", _) => ()
    _ => fail("test_escaped_characters")
  }
}

///|
fn test_edge_cases() -> Unit raise {
  // Test zero-or-more with empty string
  lexmatch b"" with longest {
    ("a*" as empty_match, _) =>
      if !(empty_match is "") {
        fail("test_edge_cases")
      }
    ...
  }

  // Test optional with no match
  lexmatch b"b" with longest {
    (("a?" as optional) ("b" as required), _) => {
      guard optional == "" && required == 'b' else {
        // CR: required extracted as single byte?  
        fail("test_edge_cases")
      }
    }
    _ => fail("test_edge_cases")
  }
}

///|
fn test_multiple_captures() -> Unit raise {
  // Test multiple named captures
  lexmatch b"first\tsecond\tthird" with longest {
    (
      ("[a-zA-Z]+" as first)
      "\t"
      ("[a-zA-Z]+" as second)
      "\t"
      ("[a-zA-Z]+" as third),
      _
    ) => {
      if !(first is "first") {
        fail("test_multiple_captures")
      }
      if !(second is "second") {
        fail("test_multiple_captures")
      }
      if !(third is "third") {
        fail("test_multiple_captures")
      }
    }
    _ => fail("test_multiple_captures")
  }
}

///|
fn test_priority_matching() -> Unit raise {
  // Test keyword vs identifier priority
  lexmatch b"if" with longest {
    ("if", _) => ()
    ("[a-zA-Z_][a-zA-Z0-9_]*" as id, _) => fail("test_priority_matching")
    _ => fail("test_priority_matching")
  }
  lexmatch b"identifier" with longest {
    ("if", _) => fail("test_priority_matching")
    ("[a-zA-Z_][a-zA-Z0-9_]*" as id, _) =>
      if !(id is "identifier") {
        fail("test_priority_matching")
      }
    _ => fail("test_priority_matching")
  }
}

///|
fn test_complex_char_classes() -> Unit raise {
  // Test alphanumeric with underscore
  lexmatch b"var_name123" with longest {
    ("[a-zA-Z0-9_]+" as alphanum, _) =>
      if !(alphanum is "var_name123") {
        fail("test_complex_char_classes")
      }
    ...
  }

  // Test non-digit pattern
  lexmatch b"abc" with longest {
    ("[^0-9]+" as non_digits, _) =>
      if !(non_digits is "abc") {
        fail("test_complex_char_classes")
      }
    ...
  }

  // Test mixed pattern
  lexmatch b"a1B2c3" with longest {
    (("[a-z]" as lower) ("[A-Z]*" as upper_star) ("[0-9]*" as digit_star), _) => {
      guard lower == 'a' && upper_star == "" && digit_star == "1" else {
        // CR: single char extracted as single byte
        fail("test_complex_char_classes")
      }
    }
    ...
  }
}

///|
fn test_hex_escapes() -> Unit raise {
  // Test hex escape sequences
  lexmatch b"ABC" with longest {
    ("\x41\x42\x43" as hex_pattern, _) =>
      if !(hex_pattern is "ABC") {
        fail("test_hex_escapes")
      }
    ...
  }
}

///|
fn test_email_pattern() -> Unit raise {
  // Test simplified email pattern
  lexmatch b"user@example.com" with longest {
    (
      ("[a-zA-Z0-9._+\-]+" as user)
      "@"
      ("[a-zA-Z0-9.\-]+" as domain)
      "\."
      ("[a-zA-Z]+" as tld),
      _
    ) => {
      guard user is "user" && domain is "example" && tld is "com" else {
        fail("test_email_pattern")
      }
    }
    _ => fail("test_email_pattern")
  }
}

///|
fn test_unicode_support() -> Unit raise {
  // Test Unicode character matching
  fn f(bytes : BytesView) -> Int {
    lexmatch bytes with longest {
      ("(?utf8:我❤️中国|𠮷)", _) => 1
      ("(?utf8:[\u3041-\u3096\u3099-\u309f]+)", _) => 2
      _ => 3
    }
  }

  assert_eq(f("我❤️中国"), 1)
  assert_eq(f("𠮷"), 1)
  assert_eq(f("あいうえお"), 2)
  assert_eq(f("abc"), 3)
}

///|
fn test_reject_utf8_overlong_encoding() -> Unit raise {
  // Test rejection of overlong UTF-8 encoding
  fn f(bytes : BytesView) -> Int {
    lexmatch bytes with longest {
      ("(?utf8:.)+", _) => 1
      _ => 2
    }
  }

  assert_eq(f("a"), 1)
  assert_eq(f("\xC1\xA1"), 2)
  assert_eq(f("\xE0\x81\xA1"), 2)
  assert_eq(f("\xC2\x99"), 1)
  assert_eq(f("\xE0\x82\x99"), 2)
}

///|
fn test_utf8_decoding() -> Unit raise {
  // Test UTF-8 decoding with regex
  fn f(bytes : BytesView) -> String raise {
    let buffer = StringBuilder::new()
    for curr = bytes {
      lexmatch curr with longest {
        ("(?utf8:.)" as c, rest) => {
          buffer.write_char(c)
          continue rest
        }
        (".", _) => raise Failure::Failure("Invalid UTF-8 sequence")
        "" => break
        _ => panic()
      }
    }
    buffer.to_string()
  }

  assert_eq(f("我❤️中国"), "我❤️中国")
  assert_eq(f("𠮷"), "𠮷")
  assert_eq(f("あいうえお"), "あいうえお")
  assert_eq(f("abc"), "abc")
  // Test handling of invalid UTF-8 sequences
  // \uD800
  assert_true(
    try f("\xED\xA0\x80") catch {
      _ => true
    } noraise {
      _ => false
    },
  )
  // \u{110000}
  assert_true(
    try f("\xF4\x90\x80\x80") catch {
      _ => true
    } noraise {
      _ => false
    },
  )
}

///|
fn test_utf8_decoding2(bytes : BytesView) -> Unit {
  // Test UTF-8 decoding with regex      
  for curr = bytes {
    lexmatch curr with longest {
      ("(?utf8:.)" as c, rest) => {
        println("Decoded character: \{c}")
        continue rest
      }
      ("." as c, rest) => {
        println("Skip Invalid UTF-8 sequence \{c}")
        continue rest
      }
      "" => break
      _ => panic()
    }
  }
}

///|
fn test_utf16_support() -> Unit raise {
  // Test UTF-16 character matching
  fn f(bytes : BytesView) -> Int {
    lexmatch bytes with longest {
      ("(?utf16le:我❤️中国|𠮷)", _) => 1
      ("(?utf16le:[\u3041-\u3096\u3099-\u309f]+)", _) => 2
      _ => 3
    }
  }

  assert_eq(f(@encoding/utf16.encode("我❤️中国")), 1)
  assert_eq(f(@encoding/utf16.encode("𠮷")), 1)
  assert_eq(f(@encoding/utf16.encode("あいうえお")), 2)
  assert_eq(f(@encoding/utf16.encode("abc")), 3)
}

///|
fn test_utf16_decoding() -> Unit raise {
  // Test UTF-16 decoding with regex
  fn f(bytes : BytesView) -> String raise {
    let buffer = StringBuilder::new()
    for curr = bytes {
      lexmatch curr with longest {
        ("(?utf16le:.)" as c, rest) => {
          buffer.write_char(c)
          continue rest
        }
        (".", _) => raise Failure::Failure("Invalid UTF-16 sequence")
        "" => break
        _ => panic()
      }
    }
    buffer.to_string()
  }

  assert_eq(f(@encoding/utf16.encode("我❤️中国")), "我❤️中国")
  assert_eq(f(@encoding/utf16.encode("𠮷")), "𠮷")
  assert_eq(f(@encoding/utf16.encode("あいうえお")), "あいうえお")
  assert_eq(f(@encoding/utf16.encode("abc")), "abc")
}

///|
fn test_minimizable_tags() -> Unit raise {
  // Test minimizable tags
  lexmatch b"a-bb-ccc" with longest {
    (("a+" as a) "-" ("b+" as b) "-" ("c+" as c), _) => println("\{a}\{b}\{c}")
    _ => fail("test_minimizable_tags")
  }
}

///|
fn test_posix_character_classes() -> Unit raise {
  let input = b"HELLO abc123_\r\n"
  lexmatch input {
    (_, "[[:lower:][:digit:]]+" as x, _) =>
      if !(x is "abc123") {
        fail("test_posix_character_classes")
      }
    _ => fail("test_posix_character_classes")
  }
  lexmatch input {
    (_, "[[:upper:]]+" as x, _) =>
      if !(x is "HELLO") {
        fail("test_posix_character_classes")
      }
    _ => fail("test_posix_character_classes")
  }
  lexmatch input {
    (_, "[[:upper:][:blank:][:lower:]]+" as x, _) =>
      if !(x is "HELLO abc") {
        fail("test_posix_character_classes")
      }
    _ => fail("test_posix_character_classes")
  }
  lexmatch input {
    (_, "[[:xdigit:]]{6}" as x, _) =>
      if !(x is "abc123") {
        fail("test_posix_character_classes")
      }
    _ => fail("test_posix_character_classes")
  }
  lexmatch input {
    (_, "[[:alnum:]\r\n]+[[:digit:]]" as x, _) =>
      if !(x is "abc123") {
        fail("test_posix_character_classes")
      }
    _ => fail("test_posix_character_classes")
  }
  lexmatch input {
    (_, "[[:word:]]+[[:blank:]][[:word:]]+" as x, _) =>
      if !(x is "HELLO abc123_") {
        println("Got: \{x}")
        fail("test_posix_character_classes")
      }
    _ => fail("test_posix_character_classes")
  }
}

///|
fn main {
  println("test_character_classes")
  println(try? test_character_classes())
  println("test_anchors")
  println(try? test_anchors())
  println("test_nested_groups")
  println(try? test_nested_groups())
  println("test_alternation")
  println(try? test_alternation())
  println("test_quantifiers")
  println(try? test_quantifiers())
  println("test_wildcard_patterns")
  println(try? test_wildcard_patterns())
  println("test_escaped_characters")
  println(try? test_escaped_characters())
  println("test_edge_cases")
  println(try? test_edge_cases())
  println("test_multiple_captures")
  println(try? test_multiple_captures())
  println("test_priority_matching")
  println(try? test_priority_matching())
  println("test_complex_char_classes")
  println(try? test_complex_char_classes())
  println("test_hex_escapes")
  println(try? test_hex_escapes())
  println("test_email_pattern")
  println(try? test_email_pattern())
  println("test_unicode_support")
  println(try? test_unicode_support())
  println("test_reject_utf8_overlong_encoding")
  println(try? test_reject_utf8_overlong_encoding())
  println("test_utf8_decoding")
  println(try? test_utf8_decoding())
  println("test_utf16_support")
  println(try? test_utf16_support())
  println("test_utf16_decoding")
  println(try? test_utf16_decoding())
  test_utf8_decoding2("我❤️中国")
  test_utf8_decoding2("𠮷")
  test_utf8_decoding2("あいうえお")
  test_utf8_decoding2("abc")
  test_utf8_decoding2("\xED\xA0\x80")
  // \u{110000}
  test_utf8_decoding2("\xF4\x90\x80\x80")
  println("Testing UTF-8 decoding with various inputs")
  test_utf8_decoding2([0x61]) // valid
  test_utf8_decoding2([0xC1, 0xA1]) // Overlong encoding
  test_utf8_decoding2([0xE0, 0x81, 0xA1]) // Overlong encoding
  test_utf8_decoding2([0XF0, 0x80, 0x81, 0xA1]) // Overlong encoding
  test_utf8_decoding2([0xC2, 0x99]) // Valid UTF-8
  test_utf8_decoding2([0xE0, 0x82, 0x99]) // Overlong encoding
  test_utf8_decoding2([0xF0, 0x80, 0x82, 0x99]) // Overlong encoding
  println("test_minimizable_tags")
  println(try? test_minimizable_tags())
  println("test_posix_character_classes")
  println(try? test_posix_character_classes())
}

///|
fn init {
  simple("a")
  simple("b")
  simple("ab")
  simple("abc")
  simple("abcd")
  a_star_capture("a")
  a_star_capture("aa")
  a_star_capture("aaa")
  a_star_capture("aaaa")
  longest_match("x")
  longest_match("le")
  longest_match("let")
  longest_match("letx")
  longest_match2("a")
  longest_match2("ab")
  longest_match2("abc")

  // Test for escaped characters
  lexmatch b"\"" with longest {
    ("\"", _) => ()
    ("\x22", _) => ()
    _ => panic()
  }

  // Run all new test functions

}
