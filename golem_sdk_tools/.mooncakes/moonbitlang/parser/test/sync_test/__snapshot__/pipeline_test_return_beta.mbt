fn f(x: Map[Int, Map[Int, Int]], a: Int, b: Int) -> Array[Int] {
    let ret = []
    x.iter().each(fn (item) {
        if item.0 != a { return }
        item.1.iter().each(fn (x) {
            if x.0 != b { return }
            ret.push(x.1)
        })
    })
    ret
}

fn g(x: Map[Int, Int], a: Int) -> Array[Int] {
    let ret = []
    x.iter().each(fn (item) {
        if item.0 != a { return }
        ret.push(item.1)
    })
    ret
}

fn h(x: Map[Int, Int], a: Option[Int]) -> Array[Int] {
    let ret = []
    x.iter().each(fn (item) {
        match a {
            Some(a) => if item.0 != a { return }
            None => ()
        }
        ret.push(item.1)
    })
    ret
}

fn main {
    let res = f({ 0: { 0: 1, 1: 2}}, 0, 1)
    println(res)
    let res = g({ 0: 1, 1: 2}, 1)
    println(res)
    let res = h({ 0: 1, 1: 2}, Some(1))
    println(res)
}
