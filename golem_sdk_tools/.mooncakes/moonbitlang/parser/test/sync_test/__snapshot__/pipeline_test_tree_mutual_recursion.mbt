enum Tree[T] {
  Empty
  Br(T, Tree[T], Tree[T])
}

fn[T : Compare + Eq] Tree::insert(self : Tree[T], v : T) -> Tree[T] {
  match self {
    Empty => Br(v, Empty, Empty)
    Br(label, left, right) => {
      if label == v {
        self
      } else if label > v {
        Br(label, left.insert(v), right)
      } else {
        Br(label, left, right.insert(v))
      }
    }
  }
}

fn[T : Compare] Tree::from_array(arr : Array[T]) -> Tree[T] {
  let mut tree = Empty
  for v in arr {
    tree = tree.insert(v)
  }
  return tree
}

fn[T] Tree::to_array(self : Tree[T]) -> Array[T] {
  let arr = []
  fn go(tree : Tree[T]) -> Unit {
    match tree {
      Empty => ()
      Br(v, left, right) => {
        go(left)
        arr.push(v)
        go(right)
      }
    }
  }
  go(self)
  return arr
}

fn[I, R] Tree::cata_empty(self : Tree[I], on_empty : () -> R, on_br : (I, R, R) -> R) -> R {
  match self {
    Empty => on_empty()
    _ => self.cata(on_empty, on_br)
  }
}

fn[I, R] Tree::cata_br(self : Tree[I], on_empty : () -> R, on_br : (I, R, R) -> R) -> R {
  match self {
    Br(label, left, right) => on_br(label, left.cata(on_empty, on_br), right.cata(on_empty, on_br))
    _ => self.cata(on_empty, on_br)
  }
}

fn[I, R] Tree::cata(self : Tree[I], on_empty : () -> R, on_br : (I, R, R) -> R) -> R {
  match self {
    Empty => self.cata_empty(on_empty, on_br)
    Br(_) => self.cata_br(on_empty, on_br)
  }
}

fn[I, R] Tree::map(self : Tree[I], f : (I) -> R) -> Tree[R] {
  fn on_empty() -> Tree[R] {
    Empty
  }
  fn on_br(label : I, left : Tree[R], right : Tree[R]) -> Tree[R] {
    Br(f(label), left, right)
  }
  self.cata(on_empty, on_br)
}

fn main {
  let tree1 : Tree[Int] = Tree::from_array([0, 1, 2, 3, 4, 5])
  let tree2 : Tree[Char] = Tree::from_array(['A', 'B', 'C', 'D', 'E'])
  println(tree1.map(fn (x) {x + x}).to_array())
  println(tree2.map(fn (c) { String::make(3, c) }).to_array())
}
