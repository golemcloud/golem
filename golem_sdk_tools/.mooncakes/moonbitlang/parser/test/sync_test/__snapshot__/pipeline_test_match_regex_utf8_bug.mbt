///|
fn main {
  let bytes: Bytes = "\xE0\xA1\x80" // "\u{0840}"
  lexmatch bytes with longest {
    "(?utf8:[\u0800-\u0fff])" => println("pass")
    _ => println("fail")
  }
}
