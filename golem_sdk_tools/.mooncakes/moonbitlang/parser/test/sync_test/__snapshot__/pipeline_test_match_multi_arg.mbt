
enum Tree {
  Nil
  Branch(Int, left~ : Tree, right~ : Tree)
}

fn Tree::top(self : Tree) -> Int? {
  match self {
    Nil => None
    Branch(top, left=_, right=_) => Some(top)
  }
}

fn Tree::iter(self : Tree, f : (Int) -> Unit) -> Unit {
  match self {
    Nil => ()
    Branch(top, left~, right~) => {
      left.iter(f)
      f(top)
      right.iter(f)
    }
  }
}

fn Tree::is_nil(self : Tree) -> Bool {
  match self {
    Branch(_) => false
    Nil => true
  }
}

fn init {
  let tree : Tree = Branch(42, left=Nil, right=Branch(0, left=Nil, right=Nil))
  println(tree.top())
  tree.iter(println)
  println(tree.is_nil())
}
