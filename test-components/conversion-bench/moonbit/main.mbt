///|
#derive.agent
struct ConversionBenchMoonbit {
  name : String
}

///|
fn ConversionBenchMoonbit::new(name : String) -> ConversionBenchMoonbit {
  { name, }
}

///|
pub fn ConversionBenchMoonbit::checksum(self : Self, input : Array[Byte]) -> UInt {
  let _ = self
  input.fold(init=0U, (sum, byte) => sum + byte.to_int().reinterpret_as_uint())
}

///|
pub fn ConversionBenchMoonbit::produce(self : Self, length : UInt) -> Array[Byte] {
  let _ = self
  Array::makei(length.reinterpret_as_int(), i => (i % 251).to_byte())
}

///|
fn main {
}
