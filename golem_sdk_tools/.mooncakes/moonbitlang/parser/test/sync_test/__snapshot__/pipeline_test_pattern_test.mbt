pub(all) enum T {
    A
    B (Int)
}

pub fn hello(x : T) -> Unit {
    match x {
        A => println(111)
        A => println(222)
        B (_) => println(333)
    }
}


pub fn hello2(x : T) -> Unit {
    match x {
        A | A  => println(111)
        A => println(222)
        B (_) => println(333)
    }
}

pub fn hello3(x : T) -> Unit {
    match x {
        A => println(444)
        B (_) => println(555)
    }
}
