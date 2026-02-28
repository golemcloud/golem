fn init {
  let a = while false {  } else { 42 }
  println(a)
  let b = for ; false ; {  } else { 42 }
  println(b)
  let c = for ; ; { break 42 } else { 0 }
  println(c)
  let d = while true { break 42 } else { 0 }
  println(d)
  let e = while false {  } else { for ; ; { break 42 } else { 0 } }
  println(e)
}
