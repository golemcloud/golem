//!build: -w -27

///|
let data1 = b"\xD3\x6B\x4B\x54\xB4\xB4\xB4\xB9\x9B\xBD\xDD\xDB\xB9\x9B\xB9\x97\x75\x55\x57\x79\x9B\xBF\xFD\xDB\xB9\x97\x75\x55\x57\x79\x9B\xBD\xDF\xFF\xFD\xDB\xB9\x97\x75\x42\x20\x00\x02\x35\x57\x79\x9B\xBD\xDF\xE0"

///|
let data2 = b"\x69\xB5\xA5\xAA\x5A\x5A\x5A\x5C\xCD\xDE\xEE\xED\xDC\xCD\xDC\xCB\xBA\xAA\xAB\xBC\xCD\xDF\xFE\xED\xDC\xCB\xBA\xAA\xAB\xBC\xCD\xDE\xEF\xFF\xFE\xED\xDC\xCB\xBA\xA1\x10\x00\x01\x1A\xAB\xBC\xCD\xDE\xEF\xF0"

///|
let data3 = b"\x34\xDA\xD2\xD5\x2D\x2D\x2D\x2E\x66\xEF\x77\x76\xEE\x66\xEE\x65\xDD\x55\x55\xDE\x66\xEF\xFF\x76\xEE\x65\xDD\x55\x55\xDE\x66\xEF\x77\xFF\xFF\x76\xEE\x65\xDD\x50\x88\x00\x00\x8D\x55\xDE\x66\xEF\x77\xF8"

///|
let data4 = b"\x1A\x6D\x69\x6A\x96\x96\x96\x97\x33\x77\xBB\xBB\x77\x33\x77\x32\xEE\xAA\xAA\xEF\x33\x77\xFF\xBB\x77\x32\xEE\xAA\xAA\xEF\x33\x77\xBB\xFF\xFF\xBB\x77\x32\xEE\xA8\x44\x00\x00\x46\xAA\xEF\x33\x77\xBB\xFC"

fn f1() -> Unit {
  match data1[:] {
    [u1(1), u2le(2), u2be(2), u3le(3), u3be(3), u8le(0x5a), u8be(0x5a), 
     u16le(0xa5a5), u16be(0xa5a5), u24le(0xeeddcc), u24be(0xeeddcc),
     u32le(0xaabbccdd), u32be(0xaabbccdd), 
     u48le(0xaabbccddeeff), u48be(0xaabbccddeeff),
     u64le(0x0011aabbccddeeff), u64be(0x0011aabbccddeeff)] => {
      println("ok")
    }
    _ => { println("not ok") }
  }
}

fn f2() -> Unit {
  match data2[:] {
    [u1(0), u1(1), u2(2), u2(2), u3(3), u3(3), u8le(0x5a), u8be(0x5a), 
     u16le(0xa5a5), u16be(0xa5a5), u24le(0xeeddcc), u24be(0xeeddcc),
     u32le(0xaabbccdd), u32be(0xaabbccdd), 
     u48le(0xaabbccddeeff), u48be(0xaabbccddeeff),
     u64le(0x0011aabbccddeeff), u64be(0x0011aabbccddeeff)] => {
      println("ok")
    }
    _ => { println("not ok") }
  }
}

fn f3() -> Unit {
  match data3[:] {
    [u2(0), u1(1), u2(2), u2(2), u3(3), u3(3), u8le(0x5a), u8be(0x5a), 
     u16le(0xa5a5), u16be(0xa5a5), u24le(0xeeddcc), u24be(0xeeddcc),
     u32le(0xaabbccdd), u32be(0xaabbccdd), 
     u48le(0xaabbccddeeff), u48be(0xaabbccddeeff),
     u64le(0x0011aabbccddeeff), u64be(0x0011aabbccddeeff)] => {
      println("ok")
    }
    _ => { println("not ok") }
  }
}

fn f4() -> Unit {
  match data4[:] {
    [u3(0), u1(1), u2(2), u2(2), u3(3), u3(3), u8le(0x5a), u8be(0x5a), 
     u16le(0xa5a5), u16be(0xa5a5), u24le(0xeeddcc), u24be(0xeeddcc),
     u32le(0xaabbccdd), u32be(0xaabbccdd), 
     u48le(0xaabbccddeeff), u48be(0xaabbccddeeff),
     u64le(0x0011aabbccddeeff), u64be(0x0011aabbccddeeff)] => {
      println("ok")
    }
    _ => { println("not ok") }
  }
}

fn main {
  f1()
  f2()
  f3()
  f4()
  println(data1.length())
}

// The OCaml program to generate the data:
// let print_hex s =
//   String.iter (fun c -> Printf.printf "\\x%02X" (Char.code c)) s;
//   print_newline ()
// ;;

// let gen_test _ =
//   for i = 0 to 3 do
//     let zeroes = Bitstring.zeroes_bitstring i in
//     let%bitstring bits =
//       {|
//           zeroes : i : bitstring;
//           true : 1;
//           2 : 2 : littleendian;
//           2 : 2 : bigendian;
//           3 : 3 : littleendian;
//           3 : 3 : bigendian;
//           0x5a : 8 : littleendian;
//           0x5a : 8 : bigendian;
//           0xa5a5 : 16 : littleendian;
//           0xa5a5 : 16 : bigendian;
//           0xeeddcc : 24 : littleendian;
//           0xeeddcc : 24 : bigendian;
//           0xaabbccdd_l : 32 : littleendian;
//           0xaabbccdd_l : 32 : bigendian;
//           0xaabbccddeeff_L : 48 : littleendian;
//           0xaabbccddeeff_L : 48 : bigendian;
//           0x0011aabbccddeeff_L : 64 : littleendian;
//           0x0011aabbccddeeff_L : 64 : bigendian
//       |}
//     in
//     print_hex (Bitstring.string_of_bitstring bits)
//   done
// ;;

// let () = gen_test ()
