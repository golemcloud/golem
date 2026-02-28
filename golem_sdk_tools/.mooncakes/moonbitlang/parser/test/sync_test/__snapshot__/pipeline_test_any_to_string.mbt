fn[T] any_to_string(obj : T) -> String = "%any.to_string"

fn[T] any_print(obj : T) -> Unit {
  println(any_to_string(obj))
}

fn[T] print_array(arr : Array[T]) -> Unit {
  for a in arr {
    any_print(a)
  }
}

struct A[T](T) derive(Show)

struct B[String](String) derive(Show)

struct C(Int)

fn main {
  print_array([A(1), A(2), A(3)])
  print_array([A(fn(x : Int) { x + 1 })])
  print_array([A((A("xxx"), B("yyy")))])
  print_array([B("xxx"), B("yyy"), B("zzz")])
  print_array([C(1), C(2), C(3)])
  print_array([A((A("xxx"), B("yyy"), C(1)))])
  any_print(D(1))
}

struct D(Int)
fn D::to_string(self : D) -> String {
  "Dxx"
}
