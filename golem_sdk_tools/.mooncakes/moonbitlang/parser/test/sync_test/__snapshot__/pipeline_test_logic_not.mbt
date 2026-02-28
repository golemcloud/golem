fn show(x : Bool) -> Unit {
    if x {
        println("true\n")
    } else {
        println("false\n")
    }
}

fn init{
    let a = true
    show(a)
    show(not(a))
    show(not(not(a)))
    show(not(not(not(a))))
}
