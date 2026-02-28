suberror StringError {
  StringError(String)
}

trait FromString {
  from_string(String) -> Self raise StringError
}

impl FromString for Bool with from_string(_self) { if (_self == "") { raise StringError("!") }; true }

fn[X : FromString] parse(s : String) -> X raise StringError {
  FromString::from_string(s)
}

fn init {
  let b : Bool = try! parse("true")
  println(b)
}
