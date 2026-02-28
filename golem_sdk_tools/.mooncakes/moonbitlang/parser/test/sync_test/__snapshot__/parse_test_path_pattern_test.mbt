fn testX(x: T) {
  match x {
    @pkg.T::Aa => println("A")
    T::x => println("X")
    T::Bbb => println("B")
    @pkg.T::Cccc => println("C")
  }
}

