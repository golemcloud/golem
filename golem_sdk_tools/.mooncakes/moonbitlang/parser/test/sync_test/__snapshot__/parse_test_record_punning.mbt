struct Single {
  r1:Int
}

struct Multi {
  r1:Int
  r2:Bool
}

struct Many {
  r1:Int
  r2:Bool
  r3:Int
  r4:Bool
}

fn init {
  let r1 = 1
  let r2 = true
  let one = 1
  let r4 = false
  let block = r1
  let single = { r1, }
  let multi = { r1, r2 }
  let multi_comma = { r1, r2 }
  let hybrid = { r1:one, r2 }
  let hybrid_comma = { r1:one, r2 }
  let no_punning = { r1:r1, r2:r2 }
  let no_punning_comma = { r1:r1, r2:r2 }
  let many = { r1, r2, r3:one, r4 }
  ()
}

