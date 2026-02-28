suberror Error1 Int

trait T {
  f1!(Self) -> Int!
  f2(Self) -> Int!
  f3!(Self) -> Int
  f4!(Self) -> Int!Error1
}

fn g1!() -> Int {
  0
}

fn g2!() -> Int! {
  0
}

fn g3!() -> Int!Error1 {
  0
}

type A Int

fn A::g3!() -> Int {
  0
}

fn A::g3!() -> Int! {
  0
}

fn A::g3!() -> Int!Error1 {
  0
}

impl T with f1!(x) {
  0
}

impl T for A with f3!(x) {
  0
}
