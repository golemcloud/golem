// Test: declare keyword parsing

// Valid uses of declare

///|
declare fn f1() -> Int

///|
declare pub fn f2() -> Int

///|
declare type T1

///|
declare pub type T2

///|
declare impl Show for T1

///|
declare pub impl Eq for T2

// Invalid uses - these should be rejected by both parsers

///|
declare suberror E1

///|
declare type! E2

///|
declare struct S1 {}

///|
declare enum En1 {}

///|
declare let x : Int = 1

///|
declare impl Show with to_string(self : Int) -> String { "" }

///|
declare impl Show for Int with to_string(self : Int) -> String { "" }

///|
declare type Alias = Int

///|
declare type Newtype Int

///|
declare impl Show with to_string(self : Int) -> String = "stub"

///|
declare impl Show for Int with to_string(self : Int) -> String = "stub"
