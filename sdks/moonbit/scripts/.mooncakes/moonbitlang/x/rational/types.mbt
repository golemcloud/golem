// Copyright 2025 International Digital Economy Academy
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

///|
/// Rational number type.
///
/// Invariants:
/// - The denominator is always positive.
/// - The numerator and denominator are always coprime.
struct Rational[T] {
  numerator : T
  denominator : T
} derive(Eq)

///|
trait Integral: Add + Sub + Mul + Div + Neg + Mod + Show + Eq + Compare + @quickcheck.Arbitrary {
  from_int(Int) -> Self
  signum(self : Self) -> Int
  abs(self : Self) -> Self
}

///|
pub impl Integral for Int64 with from_int(i : Int) -> Int64 {
  i.to_int64()
}

///|
pub impl Integral for Int64 with abs(self : Int64) -> Int64 {
  self.abs()
}

///|
pub impl Integral for Int64 with signum(self : Int64) -> Int {
  self.compare(0L)
}

///|
pub impl Integral for Int with from_int(i : Int) -> Int {
  i
}

///|
pub impl Integral for Int with abs(self : Int) -> Int {
  self.abs()
}

///|
pub impl Integral for Int with signum(self : Int) -> Int {
  self.compare(0)
}

///|
pub impl Integral for BigInt with from_int(i : Int) -> BigInt {
  BigInt::from_int(i)
}

///|
pub impl Integral for BigInt with abs(self : BigInt) -> BigInt {
  if self < 0N {
    -self
  } else {
    self
  }
}

///|
pub impl Integral for BigInt with signum(self : BigInt) -> Int {
  self.compare(0N)
}

///|
pub type Rational64 = Rational[Int64]

///|
pub type Rational32 = Rational[Int]

///|
pub type BigRational = Rational[BigInt]
