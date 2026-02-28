fn int_overload() -> Unit {
  (1 : BigInt) |> println()
  (0 : BigInt) |> println()
  (-9990_9990_9990_9990_9990 : BigInt) |> println()
  (9990_9990_9990_9990_9990 : BigInt) |> println()
  (0xFFFF_FFFF_FFFF_FFFF : BigInt) |> println()
  (0o7777_7777_7777_7777 : BigInt) |> println()
  (0b11111111_11111111_11111111_11111111 : BigInt) |> println()
}

fn literal() -> Unit {
  1N |> println()
  0N |> println()
  -9990_9990_9990_9990_9990N |> println()
  9990_9990_9990_9990_9990N |> println()
  0xFFFF_FFFF_FFFF_FFFFN |> println()
  0o7777_7777_7777_7777N |> println()
  0b11111111_11111111_11111111_11111111N |> println()
}

fn pattern(x : BigInt, expected : Int) -> Unit {
  let actual =
    match x {
      -1N => 1
      0N => 2
      1N => 3
      9990_9990_9990_9990_9990N => 4
      -9990_9990_9990_9990_9990N => 5
      0xFFFFN => 6
      0b11111111N => 7
      0o7777N => 8
      // overloaded int literal
      100 => 9
      // or pattern
      256 | 257 => 10
      // alias pattern
      0xFFFF_FFFF as bigint => {
        println(bigint)
        11
      }
      _ => 15
    }
    
  let msg = 
    if expected != actual {
      "expected " + expected.to_string() + ", but got " + actual.to_string()
    } else {
      "ok"
    }

  println(msg)
}

fn complex_pattern(x : Result[BigInt, BigInt]) -> Unit {
  match x {
    Err(1 | 2 | 3 as x) => println(x) 
    Ok(5 as x) => println("ok:\{x}")
    Ok(6 as x) | Ok(7 as x) => println("ok:\{x}")
    _ => println("fallback")
  }
}

fn init {
  int_overload()
  literal()
  pattern(-1N, 1)
  pattern(0N, 2)
  pattern(1N, 3)
  pattern(9990_9990_9990_9990_9990N, 4)
  pattern(-9990_9990_9990_9990_9990N, 5)
  pattern(0xFFFFN, 6)
  pattern(0b11111111N, 7)
  pattern(0o7777N, 8)
  pattern(100N, 9)
  pattern(256, 10)
  pattern(257, 10)
  pattern(0xFFFF_FFFFN, 11)
  pattern(-99999999999999999999999999999999999999999N, 15)
  
  complex_pattern(Err(1))
  complex_pattern(Err(2))
  complex_pattern(Err(3))
  complex_pattern(Ok(5))
  complex_pattern(Ok(6))
  complex_pattern(Ok(7))
  complex_pattern(Ok(8))
}
