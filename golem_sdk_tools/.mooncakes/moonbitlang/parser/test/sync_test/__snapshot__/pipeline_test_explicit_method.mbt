
struct T {
  x: Int
} derive(Eq, Show)

impl Add for T with add(self: T, other: T) -> T {
  { x: self.x + other.x }
}

fn init {
  println(T::add({ x: 1 }, { x: 2 }))
  println({ x: 1 } + { x: 2 })
}


impl Default for T with default() -> T {
  { x: Int::default() }
}

fn[X: Default] two_default() -> (X, X) {
  (X::default(), X::default())
}

fn init {
  let ((x: T), y) = two_default()
  println(x)
  println(y)
}

impl Mul for T with mul(self: T, other: T) -> T {
  { x: self.x * other.x }
}

fn init {
  println(T::mul({ x: 2 }, { x: 3 }))
  println({ x: 2 } * { x: 3 })
}

impl Compare for T with compare(self: T, other: T) -> Int {
  Int::compare(self.x, other.x)
}

fn init {
  println(T::compare({ x: 1 }, { x: 2 }))
  println({ x: 1 }.compare({ x: 2 }))
  println({ x: 1 } < { x: 2 })
}
