struct Foo (Int)
struct Foo (Int, Bool)
struct Foo (
  Int,
  Bool,
  String
)
struct Foo (Int,)
struct Foo (Int, Bool,)
struct Foo (
  Int,
  Bool,
  String,
)

fn local_type_decl() -> Unit {
  struct Foo (Int)
  struct Foo (Int, Bool)
  struct Foo (
    Int,
    Bool,
    String
  )
  struct Foo (Int,)
  struct Foo (Int, Bool,)
  struct Foo (
    Int,
    Bool,
    String,
  )
  ...
}