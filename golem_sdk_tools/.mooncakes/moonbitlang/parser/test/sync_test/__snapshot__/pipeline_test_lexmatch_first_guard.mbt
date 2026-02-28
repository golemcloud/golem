///|
enum FizzBuzz {
  Fizz
  Buzz
  FizzBuzz
  Number(String)
}

///|
fn fizzbuzz(n : Int) -> FizzBuzz {
  if n % 15 == 0 {
    return FizzBuzz
  } else if n % 3 == 0 {
    return Fizz
  } else if n % 5 == 0 {
    return Buzz
  } else {
    return Number(n.to_string())
  }
}

fn hack(x : StringView) -> (StringView, Int) {
  return (x, 0)
}

///|
fn first_wrong(curr : StringView) -> Int? {
  for curr = curr, i = 1 {
    lexmatch curr {
      ("[ ]+", rest) => continue rest, i
      ("[0-9]+" as n, rest) if fizzbuzz(i) is Number(x) && hack(rest) is (rest, _) && x[:] == n => continue rest, i + 1
      ("fizz", rest) if fizzbuzz(i) is Fizz => continue rest, i + 1
      ("buzz", rest) if fizzbuzz(i) is Buzz => continue rest, i + 1
      ("fizzbuzz", rest) if fizzbuzz(i) is FizzBuzz => continue rest, i + 1
      "" => return None
      _ => return Some(i)
    }
  }
}

///|
fn main {
  match first_wrong("1 2 fizz 4 buzz fizz 7 8 fizz buzz 11 fizz 13 14 fizzbuzz 16") {
    Some(i) => println("First wrong at position \{i}")
    None => println("All correct!")
  }
  match first_wrong("1 2 fizz 4 buzz fizz 7 8 fizz buzz 11 fizz 13 14 fizzbuzz fizz") {
    Some(i) => println("First wrong at position \{i}")
    None => println("All correct!")
  }
}
