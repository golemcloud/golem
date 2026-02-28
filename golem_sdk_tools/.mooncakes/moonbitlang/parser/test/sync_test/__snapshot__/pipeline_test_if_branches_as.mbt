enum E1 {
  A
} derive(Show)

enum E2 {
  B
} derive(Show)

fn f(b : Bool) -> &Show {
  if b {
    A as &Show
  } else {
    B as &Show
  }
}

fn main {
  println(f(true))
  println(f(false))
  println(f2(true))
  println(f2(false))
}

suberror A {
  Tag0
} 
impl Show for A with output(_, logger) {
  logger.write_string("A::Tag0")
}
suberror B {
  Tag0
} 
impl Show for B with output(_, logger) {
  logger.write_string("B::Tag0")
}

fn f2(b : Bool) -> Error {
  if b {
    A::Tag0
  } else {
    B::Tag0
  }
  // Note this is not considered equal
  // in core IR, due to the semantics of 
  // extensible_tag equivalence
  // see MR:6464
}