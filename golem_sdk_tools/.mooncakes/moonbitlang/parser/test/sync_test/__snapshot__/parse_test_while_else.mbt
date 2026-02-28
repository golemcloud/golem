fn init {
  while i < 0 {
    break(Some(0))
  } else {
    None
  }

  while true {
    if true {
      println(3)
    } else {
      println(4)
    }
  } else {
    println(5)
  }

  while (if true { true } else { false }) {
    println(6)
  }

  while true {
    if true {
      println(3)
    } else {
      println(4)
    }
  } else if(test) {
    println(5)
  }

  while else {
    println(7)
  }

}

fn init {
  let x = while true { x } else { y }
  let y = (while true { x } else { y }) + (while true { x })
  return(while true { x })
}

fn init {
  return(while true { x } else { y })
}

fn init {
  return while true { x } else { y }
}
