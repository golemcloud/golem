enum Tree[A] {
  Leaf
  Node (A, Tree[A], Tree[A])
}

fn[A: Show] print_tree(x: Tree[A]) -> Unit {
  match x {
    Leaf => ()
    Node(a, l, r) => { print_tree(l); println(a); print_tree(r) }
  }
}

fn init {
  let x1:Tree[Unit] = Leaf
  let x2:Tree[Char] = Node('c', Leaf, Node('d', Leaf, Leaf))
  print_tree(x2)
}

enum Expr {
  Atom(Int)
  Add(AExpr, AExpr)
}

struct AExpr {
  annotation: String
  data: Expr
}

fn sum(x: Expr) -> Int {
  match x {
    Atom(i) => i
    Add(e1, e2) => asum(e1) + asum(e2)
  }
}

fn asum(x: AExpr) -> Int {
  println(x.annotation);
  sum(x.data)
}

fn init {
  let x = { annotation: "one\n", data: Atom(1) }
  let y = { annotation: "two\n", data: Atom(2) }
  let z = { annotation: "three\n", data: Add(x, y) }
  println(asum(z))
}

struct E {
  fs: FixedArray[F]
  e: Int
}

struct F {
  es: FixedArray[E]
  f: Char
}

fn print_e(e: E) -> Unit {
  println(e.e)
  let mut i = 0
  while i < e.fs.length() {
    print_f(e.fs[i])
    i = i + 1
  }
}

fn print_f(f: F) -> Unit {
  println(f.f)
  let mut i = 0
  while i < f.es.length() {
    print_e(f.es[i])
    i = i + 1
  }
}

fn init {
  let e1 = { fs:[], e: 2023}
  let f1 = { es:[e1, e1], f: 's'}
  let f2 = { es:[e1, e1, e1], f: 't'}
  let e2 = { fs:[f1, f2], e: 929}
  print_e(e2)
}
