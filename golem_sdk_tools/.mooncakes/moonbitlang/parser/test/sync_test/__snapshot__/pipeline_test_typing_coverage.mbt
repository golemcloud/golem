enum Tree[A] {
  Leaf(A)
  Node(Tree[A], Tree[A])
} derive(Show)

type Dup[A] = (A, A)

struct TBox[A] {
  mut value : A
}

fn[T : Show] render_tree(t : Tree[T]) -> String {
  t.to_string()
}

fn[T] flatten(t : Tree[T]) -> Array[T] {
  match t {
    Leaf(x) => [x]
    Node(left, right) => {
      let l = flatten(left)
      let r = flatten(right)
      [..l, ..r]
    }
  }
}

fn[T] dup(x : T) -> Dup[T] {
  (x, x)
}

fn sum_pair(p : Dup[Int]) -> Int {
  let (a, b) = p
  a + b
}

fn bump_box(b : TBox[Int]) -> TBox[Int] {
  b.value = b.value + 1
  b
}

fn pick_opt(t : Tree[Int?]) -> Int {
  match t {
    Leaf(Some(x) as v) => {
      ignore(v)
      x
    }
    Leaf(None) => 0
    Node(a, b) => pick_opt(a) + pick_opt(b)
  }
}

fn[T] poly_id(x : T) -> T {
  x
}

fn make_counter() -> () -> Int {
  let mut n = 0
  fn () {
    n += 1
    n
  }
}

fn init {
  let t = Node(Leaf(Some(1)), Leaf(Some(2)))
  let _ = flatten(t)
  let _ = render_tree(t)
  let _ = pick_opt(t)
  let _ = sum_pair(dup(3))

  let b = TBox::{ value: 40 }
  let _ = bump_box(b)

  let counter = make_counter()
  let _ = counter()
  let _ = poly_id("ok")
  ()
}
