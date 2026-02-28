// type Iter[T] ((T) -> IterResult) -> IterResult raise?

// In this setup, the iterator itself can raise error,
// but early exit still need to be implemented via `IterResult`

fn from_bytes(b : Bytes) -> ((Byte) -> IterResult) -> IterResult raise? {
  fn (yield_) {
    for i in 0..<b.length() {
      match yield_(b[i]) {
        IterContinue => ()
        IterEnd => break IterEnd
      }
    } else {
      IterContinue
    }
  }
}

fn[X] each(
  it : ((X) -> IterResult) -> IterResult raise?,
  f : (X) -> Unit
) -> Unit raise? {
  it(fn (x) { f(x); IterContinue })
  |> ignore
}

fn[X] collect(
  it : ((X) -> IterResult) -> IterResult raise?,
) -> Array[X] raise? {
  let result = []
  each(it, result.push(_))
  result
}

fn[X] take(
  it : ((X) -> IterResult) -> IterResult raise?,
  n : Int
) -> ((X) -> IterResult) -> IterResult raise? {
  fn (yield_) {
    let mut i = 0
    it(fn (x) {
      if i >= n {
        return IterEnd
      }
      i = i + 1
      yield_(x)
    })
  }
}

fn[X, Y] map(
  it : ((X) -> IterResult) -> IterResult raise?,
  f : (X) -> Y raise?
) -> ((Y) -> IterResult) -> IterResult raise? {
  fn (yield_) {
    let mut err_from_f = None
    let result = it(fn (x) {
      try f(x) catch {
        err => {
          err_from_f = Some(err)
          IterEnd
        }
      } else {
        fx => yield_(fx)
      }
    })
    match err_from_f {
      None => result
      Some(err) => raise err
    }
  }
}

fn[X, Y] filter_map(
  it : ((X) -> IterResult) -> IterResult raise?,
  f : (X) -> Y? raise?
) -> ((Y) -> IterResult) -> IterResult raise? {
  fn (yield_) {
    let mut err_from_f = None
    let result = it(fn (x) {
      let fx = f(x) catch {
        err => {
          err_from_f = Some(err)
          return IterEnd
        }
      }
      guard fx is Some(y) else { IterContinue }
      yield_(y)
    })
    match err_from_f {
      None => result
      Some(err) => raise err
    }
  }
}

fn[X] with_last(
  it : ((X) -> IterResult) -> IterResult raise?
) -> (((X?, X)) -> IterResult) -> IterResult raise? {
  fn (yield_) {
    let mut last = None
    it(fn (x) {
      let x0 = last
      last = Some(x)
      yield_((x0, x))
    })
  }
}

suberror UTF16DecodeError {
  MissingLowSurrogate
  UnexpectedLowSurrogate
} derive(Show)

fn is_high_surrogate(x : Int) -> Bool {
  x is 0xD800..=0xDBFF
}

fn is_low_surrogate(x : Int) -> Bool {
  x is 0xDC00..=0xDFFF
}

fn decode_utf16(
  it : ((Int) -> IterResult) -> IterResult raise
) -> ((Char) -> IterResult) -> IterResult raise {
  let it = it
    |> with_last
    |> filter_map(_, fn (pair) raise {
      let (last, c) = pair
      if is_low_surrogate(c) {
        guard last is Some(c0) && is_high_surrogate(c0) else {
          raise UnexpectedLowSurrogate
        }
        Some(((c0 - 0xD800) << 10) + c - 0xDC00 + 0x10000)
      } else if last is Some(c0) && is_high_surrogate(c0) {
        raise MissingLowSurrogate
      } else if is_high_surrogate(c) {
        None
      } else {
        Some(c)
      }
    })
  map(it, _.unsafe_to_char())
}

suberror IncompleteUTF16CodePoint derive(Show)

fn bytes_to_utf16_code_points(
  it : ((Byte) -> IterResult) -> IterResult raise
) -> ((Int) -> IterResult) -> IterResult raise {
  fn (yield_) {
    let mut last = None
    let result = it(fn (b) {
      match last {
        None => {
          last = Some(b)
          IterContinue
        }
        Some(b0) => {
          last = None
          yield_((b.to_int() << 8) | b0.to_int())
        }
      }
    })
    if result is IterContinue && last is Some(_) {
      raise IncompleteUTF16CodePoint
    }
    result
  }
}

fn bytes_to_utf16(b : Bytes) -> Array[Char] raise {
  b
  |> from_bytes
  |> bytes_to_utf16_code_points
  |> decode_utf16
  |> collect
}

fn init {
  println(try? bytes_to_utf16(@encoding/utf16.encode("abc")))
  println(try? bytes_to_utf16(@encoding/utf16.encode("中文")))
  println(try? bytes_to_utf16(@encoding/utf16.encode("🤣")))
  println(try? bytes_to_utf16([ 0x00, 0xD8, 0x00, 0x61 ]))
  println(try? bytes_to_utf16([ 0x00, 0xDC ]))
}

fn bytes_arary_to_iter(
  data : Array[Bytes]
) -> ((Byte) -> IterResult) -> IterResult raise? {
  fn (yield_) {
    for i, segment in data {
      println("fetching data segment \{i + 1}")
      guard from_bytes(segment)(yield_) is IterContinue else { break IterEnd }
    } else {
      IterContinue
    }
  }
}

// laziness of iterator
fn init {
  fn run(data, n) raise {
    let it = data
    |> bytes_arary_to_iter
    |> bytes_to_utf16_code_points
    |> decode_utf16
    take(it, n) |> collect
  }
  println(try? run([ "一二", "三四", "五六", "七八", "九十" ].map(@encoding/utf16.encode(_)), 7))
  println(try? run([ "abcdefghij", "klmnopqrst", "uvwxyz" ].map(@encoding/utf16.encode(_)), 10))
}
