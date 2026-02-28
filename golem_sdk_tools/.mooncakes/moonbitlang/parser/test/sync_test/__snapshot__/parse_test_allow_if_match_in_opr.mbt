fn init {
  let a = (if true { 0 } else { 1 }) + (if false { 1 } else { 0 })
  let b = (match true { true => 1; false => 0 }) + (match false { true => 0; false => 1 })
  let c = - (if true { 1 } else { 0 })
  let d = - (match false { false => 1; true => 0 })
}