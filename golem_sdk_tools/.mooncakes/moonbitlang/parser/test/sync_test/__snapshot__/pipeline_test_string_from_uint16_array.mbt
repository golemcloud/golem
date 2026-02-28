///|
fn string_from_uint16_fixedarray(arr : FixedArray[UInt16]) -> String = "%string.unsafe_from_uint16_fixedarray"

///|
fn main {
  let arr : FixedArray[UInt16] = [
    0x0048, // 'H'
     0x0065, // 'e'
     0x006C, // 'l'
     0x006C, // 'l'
     0x006F, // 'o'
     0x002C, // ','
     0x0020, // ' '
     0x4E16, // '世'
     0x754C, // '界'
     0x0021, // '!'
  ]
  let str = string_from_uint16_fixedarray(arr)
  println(str)
  let str = string_from_uint16_fixedarray(emoji_arr)
  println(str)
  let str = string_from_uint16_fixedarray(empty_arr)
  println(str)
  let big_arr = FixedArray::make(1_000_000, (0x0048 : UInt16))
  let str = string_from_uint16_fixedarray(big_arr)
  println(str.length())
}

///|
let emoji_arr : FixedArray[UInt16] = [
  0x0048, 0x0069, 0x0020, // "Hi "
   0xD83D, 0xDC4B, // 👋 (surrogate pair)
   0x0020, // " "
   0xD83C, 0xDF89, // 🎉 (surrogate pair)
]

///|
let empty_arr : FixedArray[UInt16] = []
