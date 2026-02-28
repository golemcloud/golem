enum Tree[A] {
  Leaf
  Node (A, Tree[A], Tree[A])
}

enum Single_tree[A] {
  Empty
  TNode (A, Forest[A])
}

enum Forest[B] {
  Nothing
  More (Single_tree[B], Forest[B])
}

enum T[A, B] {
  A (Int)
  B ((A) -> A)
  C (B)
  D (T[Int, Bool])
}

let x1:Tree[Unit] = Leaf

let x2:Tree[Char] = Node('c', Leaf, Node('d', Leaf, Leaf))

let x3:Forest[Int] = More(TNode(1, Nothing), More(Empty, Nothing))

let x4:T[Unit, Unit] = A(1)

let x5:T[Int, Bool] = B(fn(x: Int) { x })

let x6:T[Unit, Char] = C('c')

let x7:T[Unit, Unit] = D(x5)

