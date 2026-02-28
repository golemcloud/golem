type T1 = Int
pub type T2[X, Y] = (X) -> Y

/// semantically invalid, but syntatically allowed
type T3 = Array[T2] derive(Show)

traitalias I1 = Show
pub traitalias I2 = Eq

// syntax error
traitalias I3 = I1[Int]

typealias @pkg.T
typealias @pkg.T[X] // syntax error
pub typealias @pkg.(T1, T2, T3)

traitalias @pkg.T
pub traitalias @pkg.(A, B, C)
traitalias @pkg.T[X] // syntax error

typealias @pkg.T as A
typealias @pkg.(
  T1,
  T2 as A
)

traitalias @pkg.T as A
traitalias @pkg.(
  T1,
  T2 as A
)

typealias Int as I
traitalias Eq as E

typealias Array[Array[Int]] as AAI
typealias Array[Array[X]] as AAX[X]
typealias (Int, Bool) as MyTuple
typealias (Int, Bool) -> Unit as MyFunction
type Bad[F[X]] = Int
