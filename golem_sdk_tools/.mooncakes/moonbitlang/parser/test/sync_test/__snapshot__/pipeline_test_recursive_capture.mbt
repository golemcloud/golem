fn fib (n : Int) -> Int {
    match n {
        0 => 0
        1 => 1
        _ =>  {
            let a = fib
            a(n - 1) + a(n - 2)
        }
    }
}

fn init {
    println(fib(10))
}