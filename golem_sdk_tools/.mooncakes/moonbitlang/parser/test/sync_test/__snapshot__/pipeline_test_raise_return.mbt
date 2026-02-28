fn f() -> Int raise {
    raise { return 42 }
}

fn main {
    println(try? f())
}
