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

// empty list, accept for completion
using @pkg {}
