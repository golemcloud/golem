///|
fn main {
  println("----- bytes ------")
  ("abcd" : Bytes) |> println // utf8("abcd") = [0x61, 0x62, 0x63, 0x64]
  ("测" : Bytes) |> println //utf8("测") = [0xe6, 0xb5, 0x8b]
  ("\xFF" : Bytes) |> println
  ("\o377" : Bytes) |> println
  //
  println("----- bytes ------")
  b"abcd" |> println // utf8("abcd") = [0x61, 0x62, 0x63, 0x64]
  b"测" |> println
  b"\xFF" |> println
  b"\o377" |> println
  //
  println("----- string ------")
  ("abcd" : String) |> println
  ("测" : String) |> println
  ("\u4E2D" : String) |> println
  ("\u{1f430}" : String) |> println
  //
  // NOTE: The multiline string cannot be overloaded to Bytes
  println("------ test complex string ------")
  "abc\u00FF__def" |> println
  "测试\u4e2Dmoonbit\n🐇12\"3456\u00D1\\\u0061" |> println
  //
  println("--------test hex sequence--------")
  ("\x01\x23\x45\x67\x89\xAB\xCD\xEF\xab\xcd\xef" : Bytes) |> println
  b"\x01\x23\x45\x67\x89\xAB\xCD\xEF\xab\xcd\xef" |> println
  //
  println("--------test oct sequence--------")
  ("\o012\o345\o007" : Bytes) |> println
  b"\o012\o345\o007" |> println
  //
  println("--------test special escape sequence--------")
  ("\n\r\t\\\"" : Bytes) |> println
  b"\n\r\t\\\"" |> println
  @encoding/utf16.encode("\n\r\t\\\"") |> println
}
