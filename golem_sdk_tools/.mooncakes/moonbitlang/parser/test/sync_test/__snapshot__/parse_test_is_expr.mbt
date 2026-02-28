fn main {
  if 1 is 2 {
    ...
  } else {
    ...
  }
  
  if x is Some(a) && a is 1 {
    ...
  } else {
    ...
  }

  let _ = h(f(x) is Some(a) && None is [_, _] || x.f() is Some(b))

  match a is C {
    true => b is Some(c)
    false => not(c is Some(d))
  }

  if (1 as Show) is _ {

  }
  let obj = (1 is _) as Show

  let range1 = a is 1..=10 + b
  let range2 = (a is 1)..=10 + b
  
}
