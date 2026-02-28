//!build: -w -68

///|
declare type T

///|
struct T {
  x : Int
}

///|
declare fn T::f(x : T) -> Int
///|
fn T::f(x : T) -> Int {
  x.x
}

///|
declare type S

///|
declare fn S::f(x : S) -> Int
///|
fn main {
  println(T::f({ x: 10 }))
}
