type MyInt Int derive(Eq, Show)

type MyHashMap[T] HashMap[Int,T]

type MyTuple[A, B] (A, B)

type MyTriple[A, B, C] (A, B, C)

type X[X] (Int, X, Int)

type X[X] ((Int, X, Int))

type Y[Y] Int[Int[Int]]

type X (Y

type Z[X (Int[X])

type Z[I[J]] (Int)
