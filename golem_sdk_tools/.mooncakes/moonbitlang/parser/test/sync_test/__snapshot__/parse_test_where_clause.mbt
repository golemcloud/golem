fn test_for_where() -> Unit {
  for i = 0; i < 10; i = i + 1 {
    println(i.to_string())
  } where {
    bound: 10,
    step: 1
  }
}

fn test_for_where_single_field() -> Unit {
  for i = 0; i < 5; i = i + 1 {
    println(i.to_string())
  } where {
    invariant: i >= 0
  }
}

fn test_for_where_no_condition() -> Unit {
  for i = 0 {
    if i >= 10 {
      break
    }
  } where {
    max_iterations: 10
  }
}
