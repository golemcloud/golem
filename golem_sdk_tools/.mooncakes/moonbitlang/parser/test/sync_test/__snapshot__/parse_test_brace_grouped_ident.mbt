
struct Foo {
  x : Int
}

let x = 2

/// record
let x2 = { x: 1 }

/// warning, should be record 
let x3 = { x; } 

/// warning, should be record
let x4 = { 
  x 
}

/// warning, should be record
let x5 = { x }  

/// record
let x6 = { x, } 

/// expression x
let x7 = { (x) } 
