struct R1 {       // abstract data type by default
  x: Int          // implicitly private field
  pub y: Int      // ERROR: `pub` field found in a abstract suberror
  priv z: Int     // WARNING: `priv` is redundant!
}

pub struct R2 {       // explicitly public struct
  x: Int              // implicitly public field
  pub y: Int          // WARNING: `pub` is redundant!
  priv z: Int         // explicitly private field
}

priv struct R3 {       // explicitly private struct
  x: Int               // implicitly private field
  pub y: Int           // ERROR: `pub` field found in a private suberror
  priv z: Int          // WARNING: `priv` is redundant!
}

enum T1 {       // abstract data type by default
  A(Int)        // implicitly private variant
  pub B(Int)    // ERROR: no individual visibility!
  priv C(Int)   // ERROR: no individual visibility!
}

pub enum T2 {       // explicitly public enum
  A(Int)            // implicitly public variant
  pub B(Int)        // ERROR: no individual visibility!
  priv C(Int)       // ERROR: no individual visibility!
}

priv enum T3 {       // explicitly private enum
  A(Int)             // implicitly private variant
  pub B(Int)         // ERROR: no individual visibility!
  priv C(Int)        // ERROR: no individual visibility!
}

pub struct S {
  x: T1  // OK
  y: T2  // OK
  z: T3  // ERROR: public field has private type `t3`!
}

// ERROR: public variable has private type `t3`!
pub let a: T3 = A(0)

// ERROR: public function has private parameter type `t3`!
pub fn f1(_x: T3) -> T1 { T1::A(0) }

// ERROR: public function has private return type `t3`!
pub fn f2(_x: T1) -> T3 { T3::A(0) }

// OK
pub fn f3(_x: T1) -> T1 { T1::A(0) }

pub fn f4(_x: T1) -> T1 {
  let _e: T3 = A(0)
  A(0)
}
