//!build: -w -27

fn f(bs: BytesView) -> Unit {
  match bs {
    [ (0x00 ..= 0x7F) as b, _ ] => println(b)
    [ u64(u), .. ] => println(u)
    _ => println("other")
  }
}

fn main {
  f([0x00, 0x01])
  f([0x80, 0x80,0x80,0x80,0x80,0x80,0x80,0x01])
  f([])
}
