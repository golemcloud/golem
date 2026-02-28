struct T {
    x : Int
} 

struct U((Int) -> Int)


fn init {
    U::U(x => x)|>ignore
    T::{ x : 2} |> ignore
}
