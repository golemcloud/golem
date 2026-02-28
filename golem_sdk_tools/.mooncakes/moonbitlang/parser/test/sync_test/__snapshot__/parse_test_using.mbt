/// docstring
using @pkg {func}

#deprecated
using @pkg {type Type}

pub using @pkg {
  func,
  Const,
  type Type,
  trait Trait,
}

using @pkg {
  func as func2,
  Const as Const2,
  type Type as Type2,
  trait Trait as Trait2,
}

// syntax error
using @pkg {
  func as Func,
  Const as c,
  type Type as ty,
  trait Trait as tr,
}

// empty list, accept for completion
using @pkg {}
