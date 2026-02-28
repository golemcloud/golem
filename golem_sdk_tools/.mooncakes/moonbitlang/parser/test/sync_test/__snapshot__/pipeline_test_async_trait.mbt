async fn[T] suspend(f : ((T) -> Unit) -> Unit) -> T noraise = "%async.suspend"
fn run_async(f : async () -> Unit noraise) -> Unit = "%async.run"

trait Stream {
  async next(Self) -> Char? noraise
}

struct Input {
  mut consumer : ((Char?) -> Unit)?
}

fn Input::new() -> Input {
  { consumer: None }
}

impl Stream for Input with next(self) {
  suspend(fn (consumer) {
    self.consumer = Some(consumer)
  })
}

suberror NoConsumer derive(Show)

fn Input::feed_char(self : Input, chr : Char) -> Unit raise {
  match self.consumer {
    None => raise NoConsumer
    Some(consumer) => {
      self.consumer = None
      consumer(Some(chr))
    }
  }
}

fn Input::terminate(self : Input) -> Unit {
  match self.consumer {
    None => ()
    Some(consumer) => {
      self.consumer = None
      consumer(None)
    }
  }
}

fn Input::feed_data(self : Input, data : String) -> Unit raise {
  for c in data {
    self.feed_char(c)
  }
}



suberror Err {
  Unmatched_Left
  Unexpected_Right
  Invalid_Char(Char)
} derive(Show)

async fn[S : Stream] is_balanced_poly(s : S) -> Unit raise Err {
  loop (0, s.next()) {
    (0, None)      => ()
    (n, None)      => raise Unmatched_Left
    (n, Some('(')) => continue (n + 1, s.next())
    (0, Some(')')) => raise Unexpected_Right
    (n, Some(')')) => continue (n - 1, s.next())
    (_, Some(c))   => raise Invalid_Char(c)
  }
}

fn run_poly(input_sequence : Array[String]) -> Unit {
  let input = Input::new()
  run_async(fn () {
    try {
      is_balanced_poly(input)
    } catch {
      err => println(err)
    } else {
      _ => println("is balanced")
    }
  })
  try {
    for data in input_sequence {
      println("sending more data ...")
      input.feed_data(data)
    }
    input.terminate()
  } catch {
    err => println("sender received error: \{err}")
  } else {
    _ => println("input closed succesfully")
  }
}

fn init {
  println("===")
  run_poly([ "(((", ")(()", ")))" ])
  println("===")
  run_poly([ "(((", ")(()", "))" ])
  println("===")
  run_poly([ "(()", "))()", "()" ])
}

// check if the parethesis in a stream is balanced
async fn is_balanced_obj(s : &Stream) -> Unit raise Err {
  loop (0, s.next()) {
    (0, None     ) => ()
    (n, None     ) => raise Unmatched_Left
    (n, Some('(')) => continue (n + 1, s.next())
    (0, Some(')')) => raise Unexpected_Right
    (n, Some(')')) => continue (n - 1, s.next())
    (_, Some(c)  ) => raise Invalid_Char(c)
  }
}

fn run_obj(input_sequence : Array[String]) -> Unit {
  let input = Input::new()
  run_async(fn () {
    try {
      is_balanced_obj(input)
    } catch {
      err => println(err)
    } else {
      _ => println("is balanced")
    }
  })
  try {
    for data in input_sequence {
      println("sending more data ...")
      input.feed_data(data)
    }
    input.terminate()
  } catch {
    err => println("sender received error: \{err}")
  } else {
    _ => println("input closed succesfully")
  }
}

fn init {
  println("===")
  run_obj([ "(((", ")(()", ")))" ])
  println("===")
  run_obj([ "(((", ")(()", "))" ])
  println("===")
  run_obj([ "(()", "))()", "()" ])
}
