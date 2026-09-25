///|
fn validate_path(path : String) -> Result[String, FilesystemToolError] {
  if path.length() == 0 {
    return Err(UnsafePath("path must not be empty"))
  }
  for c in path {
    if c == '\u0000' {
      return Err(UnsafePath("path '\{path}' must not contain NUL"))
    }
  }
  let normalized = path.trim_start(chars="/").to_owned()
  for component in normalized.split("/") {
    if component == ".." {
      return Err(
        UnsafePath("path '\{path}' contains forbidden parent traversal ('..')"),
      )
    }
  }
  if normalized == "" {
    Err(UnsafePath("path '\{path}' must identify a file"))
  } else {
    Ok(normalized)
  }
}

///|
fn valid_utf8(bytes : FixedArray[Byte]) -> Bool {
  let n = bytes.length()
  let mut i = 0
  while i < n {
    let a = bytes[i].to_int()
    if a == 0 {
      return false
    }
    if a < 0x80 {
      i += 1
    } else if a >= 0xc2 && a <= 0xdf && i + 1 < n && continuation(bytes[i + 1]) {
      i += 2
    } else if a == 0xe0 &&
      i + 2 < n &&
      between(bytes[i + 1], 0xa0, 0xbf) &&
      continuation(bytes[i + 2]) {
      i += 3
    } else if a >= 0xe1 &&
      a <= 0xec &&
      i + 2 < n &&
      continuation(bytes[i + 1]) &&
      continuation(bytes[i + 2]) {
      i += 3
    } else if a == 0xed &&
      i + 2 < n &&
      between(bytes[i + 1], 0x80, 0x9f) &&
      continuation(bytes[i + 2]) {
      i += 3
    } else if a >= 0xee &&
      a <= 0xef &&
      i + 2 < n &&
      continuation(bytes[i + 1]) &&
      continuation(bytes[i + 2]) {
      i += 3
    } else if a == 0xf0 &&
      i + 3 < n &&
      between(bytes[i + 1], 0x90, 0xbf) &&
      continuation(bytes[i + 2]) &&
      continuation(bytes[i + 3]) {
      i += 4
    } else if a >= 0xf1 &&
      a <= 0xf3 &&
      i + 3 < n &&
      continuation(bytes[i + 1]) &&
      continuation(bytes[i + 2]) &&
      continuation(bytes[i + 3]) {
      i += 4
    } else if a == 0xf4 &&
      i + 3 < n &&
      between(bytes[i + 1], 0x80, 0x8f) &&
      continuation(bytes[i + 2]) &&
      continuation(bytes[i + 3]) {
      i += 4
    } else {
      return false
    }
  }
  true
}

///|
fn continuation(byte : Byte) -> Bool {
  between(byte, 0x80, 0xbf)
}

///|
fn between(byte : Byte, low : Int, high : Int) -> Bool {
  let value = byte.to_int()
  value >= low && value <= high
}

///|
fn line_ranges(bytes : FixedArray[Byte]) -> Array[(Int, Int)] {
  let lines : Array[(Int, Int)] = []
  if bytes.length() == 0 {
    return lines
  }
  let mut start = 0
  for i in 0..<bytes.length() {
    if bytes[i] == b'\n' {
      lines.push((start, i + 1))
      start = i + 1
    }
  }
  if start < bytes.length() {
    lines.push((start, bytes.length()))
  }
  lines
}

///|
fn slice_lines(
  bytes : FixedArray[Byte],
  start_line : UInt64?,
  end_line : UInt64?,
) -> Result[(FixedArray[Byte], UInt64, UInt64?, UInt64?), FilesystemToolError] {
  if start_line is Some(0UL) || end_line is Some(0UL) {
    return Err(InvalidRange("line numbers are 1-based"))
  }
  if start_line is Some(start) && end_line is Some(end) && start > end {
    return Err(InvalidRange("start_line must not exceed end_line"))
  }
  let ranges = line_ranges(bytes)
  let total = ranges.length().to_uint64()
  let first = start_line.unwrap_or(1UL)
  let last = end_line.unwrap_or(total)
  if start_line is Some(_) && first > total {
    return Err(InvalidRange("start_line \{first} exceeds total lines \{total}"))
  }
  let from = if total == 0UL { 0 } else { (first - 1UL).to_int() }
  let through = if last > total { total.to_int() } else { last.to_int() }
  let begin = if from < ranges.length() { ranges[from].0 } else { 0 }
  let finish = if through > from { ranges[through - 1].1 } else { begin }
  let selected = FixedArray::make(finish - begin, b'\x00')
  for i in begin..<finish {
    selected[i - begin] = bytes[i]
  }
  let effective_end = if last < total { last } else { total }
  Ok(
    (
      selected,
      total,
      if total == 0UL {
        None
      } else {
        Some(first)
      },
      if total == 0UL {
        None
      } else {
        Some(effective_end)
      },
    ),
  )
}

///|
fn find_matches(haystack : String, needle : String) -> Array[Int] {
  let matches : Array[Int] = []
  if needle == "" || needle.length() > haystack.length() {
    return matches
  }
  let mut offset = 0
  while offset <= haystack.length() - needle.length() {
    match haystack[offset:].find(needle) {
      None => break
      Some(relative) => {
        let found = offset + relative
        matches.push(found)
        offset = haystack
          .offset_of_nth_char(1, start_offset=found)
          .unwrap_or(haystack.length())
      }
    }
  }
  matches
}
