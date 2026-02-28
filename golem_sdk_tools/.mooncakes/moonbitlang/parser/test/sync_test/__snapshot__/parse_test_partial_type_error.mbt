struct Stock {
    price : Int 
    name : String
}

enum Lst[T] {
    Nil 
    Cons(Cell[T])
}

struct Cell[T] {
    data : T
    next : Lst [T]
}

fu
