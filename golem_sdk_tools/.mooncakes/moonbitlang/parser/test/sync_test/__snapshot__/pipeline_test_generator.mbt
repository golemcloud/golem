//!build: -w -20

async fn[T] callcc(cb : ((T) -> Unit) -> Unit) -> T noraise = "%async.suspend"
fn run_async(f : async () -> Unit noraise) -> Unit = "%async.run"

struct Generator[Elem] {
  mut value : Elem?
  mut next : () -> Unit
}

fn[Elem] Generator::spawn(f : async (Generator[Elem]) -> Unit noraise) -> Generator[Elem] {
  let gen = { value: None, next: panic }
  gen.next = fn () {
    run_async(fn () {
      f(gen)
      gen.value = None
      // [code] will probably capture [coro] itself, forming a cycle,
      // manually break the cycle here so that the memory can be free'ed 
      gen.next = panic
    })
  }
  gen
}

async fn[Elem] Generator::produce_value(
  self : Generator[Elem],
  value : Elem,
) -> Unit noraise {
  callcc(fn (k) {
    self.value = Some(value)
    self.next = fn () { k(()) }
  })
}

fn[Elem] Generator::next(self : Generator[Elem]) -> Elem? {
  (self.next)()
  self.value
}

fn[Elem] Generator::iter(self : Generator[Elem]) -> Iter[Elem] {
  Iter::new(() => self.next())
}

fn init {
  let gen = Generator::spawn(fn (self) {
    for i = 0; i < 10; i = i + 1 {
      self.produce_value(i)
    }
  })
  println(gen.iter())
}
