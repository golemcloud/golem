
let callbacks : FixedArray [(Int) -> Int] = [fib2]

fn fib2 (n : Int) -> Int {
    match n {
        0 => 0
        1 => 1
        _ =>  {
            let a = fib2
            // here its fvs are [fib2] which is empty
            // if we consider it recurisve
            a(n - 1) + a(n - 2)
        }
    }
}

fn init {
    println(fib2(10))

    println(callbacks[0](10))
}
