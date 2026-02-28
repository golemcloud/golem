fn[T : Hash] stable_hash(value : T) -> Int {
  let hasher = Hasher::new(seed = 42)
  hasher.combine(value)
  hasher.finalize()
}

fn[X : Show + Hash] show_hash(x : X) -> Unit {
  let h = stable_hash(x)
  println("hash(\{x}) => \{h}")
}

enum E {
  A
  B(Int)
  C(Int)
  D(E)
} derive(Show, Hash)

fn init {
  show_hash(A)
  show_hash(B(42))
  show_hash(C(42))
  show_hash(D(D(A)))
  show_hash(D(D(D(A))))
}

struct R1 {
  x : Int
  y : Int
} derive(Show, Hash)

struct R2[X, Y] {
  x : X
  y : Y
} derive(Show, Hash)

fn init {
  show_hash(R1::{ x: 1, y: 2 })
  show_hash(R2::{ x: 1, y: 2 })
  show_hash(R2::{ x: 2, y: 1 })
}

struct NT(R2[E, E]) derive(Show, Hash)

fn init {
  show_hash(R2::{ x: A, y: D(B(42)) })
  show_hash(NT({ x: A, y: D(B(42)) }))
}
