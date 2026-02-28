fn init{
  match Option::Some(Option::Some(5)) {
    // the precedence of `|` is higher than `as`
    Some(_) | None as x => println(x) 
  }
  match Option::Some(Option::Some(5)) {
    Some(Some(_)) | Some(None) | None as x => println(x)
  }
}

fn init{
  match @immut/list.of([1,2,3]) {
    // rare case
    (Nil as xs) | (Cons(_, _) as xs) => println(xs)  
  }
}

/// Hard to handle by handwritten parser.
/// Consider to make it invalid.
fn init{
  match @immut/list.of([1,2,3]) {
    // error: xs is bound more than once
    Nil as xs | Cons(_, _) as xs => println(xs)  
  }

  match @immut/list.of([1,2,3]) {
    Nil as xs | (Cons(_, _) as xs) => println(xs)  
  } 
}

fn github_core_lex_string() {
  match Option::Some('f') {
   Some('-' | '+' as c) => ()
  }
}

fn init {
  match 10 {
    10 | 11 as a as b as c => ()
  }
}
