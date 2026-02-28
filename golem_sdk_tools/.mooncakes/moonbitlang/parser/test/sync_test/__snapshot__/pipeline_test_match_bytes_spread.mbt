///|
const B : Bytes = "中文"

///|
fn f(b : Bytes) -> Unit {
  match b {
    [.. B, .. rest] => println(rest.to_string())
    [.. start, .. b"xyz"] => println(start.to_string())
    _ => println("other")
  }
}

///|
fn main {
  f(b"abc")
  f("中文你好")
  f("你好xyz")
  let bytes2 : Bytes = "你好,ok"
  utf8_bytes_pattern_test(bytes2)
}

fn utf8_bytes_pattern_test(b : Bytes) -> Unit {
  match b {
    [.. "你好", .. ok] => println(ok)
    _ => println("Doesn't start with \"你好\"")
  }
}
