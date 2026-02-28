
struct T {
  x: Int
  y: Int
} derive(Eq, Show)

type U derive(@pkg.Interface)

enum R {
  A
  B
} derive(Hash)

struct S {} derive
