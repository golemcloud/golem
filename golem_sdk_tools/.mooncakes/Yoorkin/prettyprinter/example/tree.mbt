///|
enum Tree[A] {
  Leaf(A)
  Node(Array[Tree[A]])
}

///|
pub impl[A : @pp.Pretty] @pp.Pretty for Tree[A] with pretty(tree : Tree[A]) {
  match tree {
    Leaf(x) => x.pretty()
    Node(xs) =>
      @pp.group(
        @pp.text("Node(") +
        @pp.nest(
          @pp.line +
          @pp.separate(@pp.char(',') + @pp.line, xs.map(@pp.Pretty::pretty)),
        ) +
        @pp.line +
        @pp.char(')'),
      )
  }
}

///|
test {
  let tree = Node([
    Leaf(100),
    Leaf(200),
    Leaf(300000000),
    Node([
      Leaf(400),
      Leaf(500),
      Node([Leaf(6000000), Leaf(7000000), Leaf(80000000), Leaf(80000001)]),
      Leaf(900),
    ]),
    Leaf(1000),
  ])
  inspect(
    @pp.render(tree, width=50),
    content=(
      #|Node(
      #|  100,
      #|  200,
      #|  300000000,
      #|  Node(
      #|    400,
      #|    500,
      #|    Node( 6000000, 7000000, 80000000, 80000001 ),
      #|    900
      #|  ),
      #|  1000
      #|)
    ),
  )
}
