fn escape_in_char() -> Unit {
  println(repr('\u{5154}'))
  println(repr('\u5154'))
  println(repr('\u{4D}'))
  println(repr('\n'))
  println(repr('\r'))
  println(repr('\b'))
  println(repr('\t'))
  println(repr('\\'))
  println(repr('\f'))
  println(repr('\/'))
}

fn escape_in_string() -> Unit {
  println(repr("\u{5154}"))
  println(repr("\u5154"))
  println(repr("\u{4D}"))
  println(repr("\n"))
  println(repr("\r"))
  println(repr("\b"))
  println(repr("\t"))
  println(repr("\\"))
  println(repr("\f"))
  println(repr("\/"))
}

fn escape_in_byte() -> Unit {
  println(b'\o115')
  println(b'\n')
  println(b'\r')
  println(b'\b')
  println(b'\t')
  println(b'\\')
  println(b'\f')
  println(b'\/')
}

fn escape_in_bytes() -> Unit {
  println(b"\o115")
  println(b"\n")
  println(b"\r")
  println(b"\b")
  println(b"\t")
  println(b"\\")
  println(b"\f")
  println(b"\/")
}

fn main {
  println("escape in char:")
  escape_in_char()
  println("escape in byte:")
  escape_in_byte()
  println("escape in string:")
  escape_in_string()
  println("escape in bytes:")
  escape_in_bytes()
}