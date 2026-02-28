async fn[T] callcc(cb : ((T) -> Unit) -> Unit) -> T noraise = "%async.suspend"

fn run_async(f : async () -> Unit noraise) -> Unit = "%async.run"

///|
struct Generator[Elem] {
  mut value : Elem?
  mut next : () -> Unit
}

///|
fn[Elem : Show] print_generator(gen : Generator[Elem]) -> Unit {
  // we need to kick it off
  (gen.next)()
  while true {
    match gen.value {
      None => return
      Some(v) => {
        println(v)
        (gen.next)()
      }
    }
  }
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

///|
enum Tree {
  Leaf
  Node(Tree, Int, Tree)
}

///|
fn tree_to_iter(tree : Tree) -> AIter[Int] noraise {
  AIter(async fn(yield_)noraise {
    match tree {
      Leaf => IterContinue
      Node(left, value, right) => {
        if (tree_to_iter(left).0)(yield_) == IterEnd {
          return IterEnd
        }
        if yield_(value) == IterEnd {
          return IterEnd
        }
        if (tree_to_iter(right).0)(yield_) == IterEnd {
          return IterEnd
        }
        IterContinue
      }
    }
  })
}

///|
fn tree_generator(tree : Tree) -> Generator[Int] {
  async fn walk(gen: Generator[Int], tree: Tree) -> Unit noraise {
    match tree {
      Tree::Leaf => ()
      Tree::Node(left, value, right) => {
        walk(gen, left)
        callcc(fn (k) {
          gen.value = Some(value)
          gen.next = fn () { k(()) }
        })
        walk(gen, right)
      }
    }
  }
  Generator::spawn(fn (self) {
    walk(self, tree)
  })
}

struct AIter[T](async (async (T) -> IterResult noraise) -> IterResult noraise)

async fn[T] AIter::run(self : AIter[T], f : async (T) -> IterResult noraise) -> IterResult noraise {
  (self.0)(f)
}

fn[Elem] from_iter(iter: AIter[Elem]) -> Generator[Elem] {
  Generator::spawn(fn (self) {
    iter.run(fn (v) {
      callcc(fn (k) {
        self.value = Some(v)
        self.next = fn () { k(IterContinue) }
      })
    }) |> ignore
  })
}

///|
fn init {
  let tree = Tree::Node(
    Node(Node(Leaf, 0, Leaf), 1, Node(Leaf, 2, Leaf)),
    3,
    Node(Leaf, 4, Node(Leaf, 5, Leaf)),
  )
  let gen = tree_generator(tree)
  print_generator(gen)
  let mut iter = None
  run_async(fn() { iter = Some(tree_to_iter(tree)) })
  match iter {
    None => println("wrong")
    Some(iter) => print_generator(from_iter(iter))
  }
}
