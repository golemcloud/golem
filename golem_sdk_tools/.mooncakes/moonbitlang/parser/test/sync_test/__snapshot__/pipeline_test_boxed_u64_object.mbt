trait Foo {
  foo(Self) -> String
}

impl Foo for UInt64 with foo(self : UInt64) -> String {
  "foo"
}

fn box(x : &Foo) -> String {
  x.foo()
}

fn init {
  println(box(42UL as &Foo))
  println(box(42UL))
}


