fn add(x : Int, y : Int) -> Int {
  x + y
}

fn mul2(x : Int) -> Int {
  x * 2
}

fn init {
  10 
  |> mul2() |> add(10) 
  |> println
}

fn int_to_string(x : Int) -> String {
  "\"" + x.to_string() + "\""
}

fn init {
  let a = 5 |> mul2() |> add(5) |> int_to_string()
  println(a)
}


fn init {
  // println((3.5 |> Double::to_string))
  println((1 |> Int::compare(5)))
  println("some string" |> Eq::equal("some string"))
}

fn[A,B] f(x : A, g : (A) -> B) -> B { g(x) }
fn g_str(x : String) -> String { x }

fn init {
  5 |> f(mul2) |> println
  "str" |> f(g_str) |> println
  42 |> Option::Some |> println
  let opt: String? = "x" |> Some
  println(opt)
}

fn want_option(x : Int?) -> Unit {
  println(x)
}

fn[X] id(x : X) -> X { x }

fn init {
  Some(42) |> want_option
  let opt : _? = Some(42) |> id
  println(opt)
}
