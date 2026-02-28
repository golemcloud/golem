suberror Err derive(Show)

fn array_view(arr : Array[Int], coin : Bool) -> ArrayView[Int] raise Err {
  if coin {
    raise Err
  }
  arr[:]
}

fn bytes_view(bytes : Bytes, coin : Bool) -> BytesView raise Err {
  if coin {
    raise Err
  }
  bytes[:]
}

fn string_view(str : String, coin : Bool) -> StringView raise Err {
  if coin {
    raise Err
  }
  str[:]
}

fn main {
  (try? array_view([1, 2, 3], true)) |> println
  (try? array_view([1, 2, 3], false)) |> println
  (try? bytes_view("abc", true)) |> println
  (try? bytes_view("abc", false)) |> println
  (try? string_view("abc", true)) |> println
  (try? string_view("abc", false)) |> println
}
