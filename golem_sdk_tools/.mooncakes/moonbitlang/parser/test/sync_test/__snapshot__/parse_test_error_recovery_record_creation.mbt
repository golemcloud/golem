struct Foo {
  x : Int  
  y : Int
}

struct foo {
  x : Int
}

test "if bool {...}" {
  if Foo { ... }
  match Foo { ... }
  while Foo { ... }
  for ;; Foo { ... }
  loop Foo { ... }
}

test "if bool { f(x) }" {
  let foo = false
  if foo { f(x) } 
  if Foo { f(x) } 
  while Foo { f(x) } 
  for x=0; cond; x=Foo { f(x)}
}

test "if bool {}" {
  let foo = false
  if foo {}
  if Foo {}
  while Foo {}
  for x=0;; x=Foo {}
}

test "if bool {x}" {
  let foo = false
  let x = 1
  let _ = if foo {x}
  let _ = if Foo {x}
  let _ = while Foo {x}
  let _ = for x=0; cond; x=Foo {x}
}

test "recovery: Foo {x, }" {
  let a = Foo {x,}
  let b = loop Foo{x,} { ... }
  let c = while Foo{x,} { ... }
  let d = for x=0;cond;x=Foo{x,} {...}
}

test "recovery: Foo {x,y}" {
  let a2 = Foo {x,y}
  let b2 = loop Foo{x,y} { ... }
  let c2 = while Foo{x,y} { ... }
  let d2 = for x=0;cond;x=Foo{x,y} {...}
} 

test "recovery: Foo {x:v, y:v}" {
  let a2 = Foo {x:v,y:v}
  let b2 = loop Foo{x:v,y:v} { ... }
  let c2 = while Foo{x:v,y:v} { ... }
  let d2 = for x=0;cond;x=Foo{x:v,y:v} {...}
}

test "recovery: Foo {..rcd}" {
  let a3 = Foo {..rcd}
  let b3 = loop Foo{..rcd} { ... }
  let c3 = while Foo{..rcd} { ... }
  let d3 = for x=0;cond;x=Foo{..rcd} {...}
}

