let x = 1

fn add (y:Int) -> Int {
    y + x
}


fn call_add(z : Int) -> Int {
    add(add(z))
}

fn init {
    println(call_add(2))
}