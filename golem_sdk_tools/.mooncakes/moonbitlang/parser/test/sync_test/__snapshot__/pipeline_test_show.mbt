pub(all) struct Foo[T] {
  foo:T
}

pub impl[T: Show] Show for Foo[T] with output(self, logger) {
  self.foo.output(logger)
}

pub(all) struct Bar[T, S] {
  bar:T
  baz:S
}

pub impl[T: Show, S: Show] Show for Bar[T, S] with output(self, logger) {
  logger.write_string("\nhe")
  logger.write_string(self.bar.to_string())
  logger.write_string("\nworld")
  logger.write_string(self.baz.to_string())
}

pub(all) enum Opt[T] {
  Nothing
  Just(T)
}

pub fn[T: Show] Opt::to_string(self: Opt[T]) -> String {
  match self {
    Nothing => "nothing"
    Just(x) => x.to_string()
  }
}

fn init {
  let x: Foo[_] = { foo: 1 }
  let ox: Opt[_] = Just(x)
  println(ox.to_string())
  let y: Bar[_] = { bar: "llo", baz: '!'}
  let oy: Opt[_] = Just(y)
  println(oy.to_string())
}
