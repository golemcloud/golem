///|
test "line slicing preserves LF and CRLF" {
  let bytes = Bytes::to_fixedarray(b"one\r\ntwo\nthree")
  match slice_lines(bytes, Some(1UL), Some(2UL)) {
    Ok((selected, total, start, end)) => {
      assert_eq(Bytes::from_array(selected), b"one\r\ntwo\n")
      assert_eq(total, 3UL)
      assert_eq(start, Some(1UL))
      assert_eq(end, Some(2UL))
    }
    Err(_) => fail("expected valid slice")
  }
}

///|
test "empty file has zero lines" {
  match slice_lines(FixedArray::make(0, b'\x00'), None, None) {
    Ok((selected, total, start, end)) => {
      assert_eq(selected.length(), 0)
      assert_eq(total, 0UL)
      assert_eq(start, None)
      assert_eq(end, None)
    }
    Err(_) => fail("expected valid empty slice")
  }
}

///|
test "binary detection rejects NUL and invalid UTF-8" {
  assert_false(valid_utf8(Bytes::to_fixedarray(b"a\x00b")))
  assert_false(valid_utf8([0xff]))
  assert_true(valid_utf8(Bytes::to_fixedarray(b"hello\xe2\x82\xac")))
}

///|
test "unsafe path detection rejects traversal" {
  assert_true(validate_path("../secret") is Err(_))
  assert_true(validate_path("safe/../secret") is Err(_))
  assert_true(validate_path("safe/file") is Ok("safe/file"))
  assert_true(validate_path("/safe/file") is Ok("safe/file"))
}

///|
test "edit matching distinguishes stale and ambiguity" {
  assert_eq(find_matches("alpha beta", "missing").length(), 0)
  assert_eq(find_matches("short", "a longer needle").length(), 0)
  assert_eq(find_matches("alpha beta alpha", "alpha").length(), 2)
  assert_eq(find_matches("alpha beta", "alpha").length(), 1)
  assert_eq(find_matches("aaa", "aa"), [0, 1])
  assert_eq(find_matches("🤣🤣🤣", "🤣🤣"), [0, 2])
}
