
enum T {
  Hey(x~ : Int, Int)
  Hi(y~ : Int)
}


pub fn test_T(x : T) -> Int {
  match x {
    Hey(o, ..) => o
    Hi (_) => 2
  }
}

fn init {
  T::Hey(3,x=2) |> test_T |> println
  T::Hi(y=2) |> test_T |> println
}
