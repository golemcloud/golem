struct Tree {
  children : FixedArray[Tree]
}

let x : Tree = { children: [] }

fn init {
  println(x.children.length())
}
