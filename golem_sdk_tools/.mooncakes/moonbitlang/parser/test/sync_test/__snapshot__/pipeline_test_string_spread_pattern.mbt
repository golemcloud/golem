///|
fn f1(s: String) -> Unit {
  match s {
    [ .."hello", ..rest ] => println(rest)
    [..rest, .."world"] => println(rest)
    [.."0x", '0', ..rest, '0', .."x0"] => println(rest)
    [.."yy", .."zz"] => println("yyzz")
    [..""] => println("empty")
    _ => println("other")
  }
}

fn f2(s: StringView) -> Unit {
  match s {
    [ .."hello", ..rest ] => println(rest)
    [..rest, .."world"] => println(rest)
    [.."0x", '0', ..rest, '0', .."x0"] => println(rest)
    [.."yy", .."zz"] => println("yyzz")
    [..""] => println("empty")
    _ => println("other")
  }
}

///|
fn f3(s : String) -> Unit {
  match s {
    [.."😭😭😭", ..rest] => println(rest)
    [..rest, .."😭😭😭"] => println(rest)
    _ => println("other")
  }
}

///|
fn main {
  f1("hello world")
  f1("xhello world")
  f1("0x0x0x0x0x0x0")
  f1("yyzz")
  f1("yyzz0x0")
  f1("")
  f2("hello world".view())
  f2("xhello world".view())
  f2("0x0x0x0x0x0x0".view())
  f2("yyzz".view())
  f2("yyzz0x0".view())
  f2("".view())
  f3("😭😭😭😂")
  f3("😂😭😭😭")
  f3("")
}
