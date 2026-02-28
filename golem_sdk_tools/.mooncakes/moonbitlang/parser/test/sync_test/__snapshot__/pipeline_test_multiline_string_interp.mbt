//!build: -w -27

fn template2() -> String {
  $| line1
  $| line2
  $| line3
  $| interp1 \{'a'} interp2 \{'b'} interp3
  $| line4
  $| line5
  $| interp4 \{'c'} interp5 \{'d'}
  $| interp6 \{'e'} interp7 \{'f'} interp8
  $|
}

fn all_raw_string() -> String {
  #| line1
  #| line2
  #|
  #| line3
  #| line4
  #| line5
}

fn all_raw_string_with_trailing_newline() -> String {
  #| line1
  #| line2
  #| line3
  #| line4
  #| line5
  #|
}

fn all_interp_string() -> String {
  $| interp1 \{'a'} interp2 \{'b'} interp3
  $|
  $| interp4 \{'c'} interp5 \{'d'} 
  $| interp6 \{'e'} interp7 \{'f'} 
}

fn all_interp_string_with_trailing_newline() -> String {
  $| interp1 \{'a'}\{'b'}\{'c'} interp2 interp3
  $| interp4 \{'c'} interp5 \{'d'} 
  $| interp6 \{'e'} interp7 \{'f'} 
  $|
}

fn empty_raw_string() -> String {
  #|
}

fn empty_interp_string() -> String {
  $|
}

/// toplevel multiline string without annotation. 
/// Typer will infer the type.
let str1 = 
  #| Hello, 
  #| World.

let str2 =
  $| Hello \{str1},
  $| World.

fn interp_with_dict() -> String {
  let dict = {
    "key1": 100, 
    "key2": 200,
  }
  let r = {
    $| key1: \{dict["key1"]} 
    $| key2: \{dict["key2"]}
  }
  r
}

fn main {
  println(template2())
  println("---------------------------------------")
  println(all_raw_string())
  println("---------------------------------------")
  println(all_raw_string_with_trailing_newline())
  println("---------------------------------------")
  println(all_interp_string())
  println("---------------------------------------")
  println(all_interp_string_with_trailing_newline())
  println("---------------------------------------")
  println(empty_raw_string())
  println("---------------------------------------")
  println(empty_interp_string())
  println("---------------------------------------")
  println(interp_with_dict())
}
