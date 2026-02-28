fn escape_in_char() -> Unit {
  let _ = '\u{5154}'
  let _ = '\u5154'
  let _ = '\o115'
  let _ = '\n'
  let _ = '\r'
  let _ = '\b'
  let _ = '\t'
  let _ = '\\'
  let _ = '\f'
  let _ = '\/'
}

fn escape_in_string() -> Unit {
  let _ = "\u{5154}"
  let _ = "\u5154"
  let _ = "\o115"
  let _ = "\n"
  let _ = "\r"
  let _ = "\b"
  let _ = "\t"
  let _ = "\\"
  let _ = "\f"
  let _ = "\/"
}

fn escape_in_byte() -> Unit {
  let _ = b'\o115'
  let _ = b'\n'
  let _ = b'\r'
  let _ = b'\b'
  let _ = b'\t'
  let _ = b'\\'
  let _ = b'\f'
  let _ = b'\/'
}

fn escape_in_bytes() -> Unit {
  let _ = b"\o115"
  let _ = b"\n"
  let _ = b"\r"
  let _ = b"\b"
  let _ = b"\t"
  let _ = b"\\"
  let _ = b"\f"
  let _ = b"\/"
}

fn escape_in_string_interpolation() -> Unit {
  let a = "_"
  println("\\ \u{5154} \u5154 \{a} \o115 \x42 \n") 
}

fn short_unicode_eacape() -> Unit {
  println("\u{8}abc")
  println('\u{8}')
}

