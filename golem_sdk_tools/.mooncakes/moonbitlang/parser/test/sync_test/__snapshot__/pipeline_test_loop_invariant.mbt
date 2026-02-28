///|
fn test_loop_invariant_basic() -> Unit {
  for i = 0; i < 10; i = i + 1 {
    println(i.to_string())
  } where {
    invariant: i >= 0,
    reasoning: "i starts at 0 and increments by 1 each iteration",
  }
}

///|
fn test_loop_invariant_multiple() -> Unit {
  for i = 0, sum = 0; i < 5; i = i + 1, sum = sum + i {
    println(sum.to_string())
  } where {
    invariant: i >= 0,
    invariant: sum >= 0,
    reasoning: "i and sum are always non-negative",
  }
}

///|
fn test_loop_invariant_bound() -> Unit {
  let n = 10
  for i = 0; i < n; i = i + 1 {
    println(i.to_string())
  } where {
    invariant: i <= n,
    reasoning: "i is always less than or equal to n",
  }
}

///|
fn test_loop_invariant_decreasing() -> Unit {
  for i = 10; i > 0; i = i - 1 {
    println(i.to_string())
  } where {
    invariant: i >= 0,
    reasoning: "i is always non-negative and decreases towards 0",
  }
}

///|
fn test_loop_invariant_with_array() -> Unit {
  let arr = [1, 2, 3, 4, 5]
  for i = 0; i < arr.length(); i = i + 1 {
    println(arr[i].to_string())
  } where {
    invariant: i >= 0,
    invariant: i <= arr.length(),
    reasoning: "i is a valid index for the array",
  }
}

///|
fn init {
  test_loop_invariant_basic()
  test_loop_invariant_multiple()
  test_loop_invariant_bound()
  test_loop_invariant_decreasing()
  test_loop_invariant_with_array()
}
