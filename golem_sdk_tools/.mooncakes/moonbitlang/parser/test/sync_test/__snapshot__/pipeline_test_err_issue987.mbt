fn init {
  let x = (1,2)
  let (1, a) = x
}

struct PersonalInfo {
  name : String
  age : Int
}

fn init {
  let info = { name: "Moonbit", age: 1 }
  let { age: 0, name } = info
  println("What?")
}
