// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1.

///|
priv struct StrictJsonScanner {
  source : String
  mut index : Int
}

///|
fn StrictJsonScanner::skip_whitespace(self : StrictJsonScanner) -> Unit {
  while self.index < self.source.length() {
    match self.source[self.index] {
      ' ' | '\n' | '\r' | '\t' => self.index += 1
      _ => return
    }
  }
}

///|
fn StrictJsonScanner::take(self : StrictJsonScanner, expected : UInt16) -> Unit raise {
  self.skip_whitespace()
  guard self.index < self.source.length() &&
    self.source[self.index] == expected else {
    raise BridgeError("malformed-message: invalid JSON structure")
  }
  self.index += 1
}

///|
fn StrictJsonScanner::scan_string(self : StrictJsonScanner) -> String raise {
  self.skip_whitespace()
  let start = self.index
  self.take('"')
  let mut escaped = false
  while self.index < self.source.length() {
    let char = self.source[self.index]
    self.index += 1
    if escaped {
      escaped = false
    } else if char == '\\' {
      escaped = true
    } else if char == '"' {
      let literal = self.source[start:self.index].to_string()
      return expect_string(@json.parse(literal))
    }
  }
  raise BridgeError("malformed-message: unterminated JSON string")
}

///|
fn StrictJsonScanner::scan_primitive(self : StrictJsonScanner) -> Unit raise {
  self.skip_whitespace()
  let start = self.index
  while self.index < self.source.length() {
    match self.source[self.index] {
      ' ' | '\n' | '\r' | '\t' | ',' | ']' | '}' => break
      _ => self.index += 1
    }
  }
  if self.index == start {
    raise BridgeError("malformed-message: missing JSON value")
  }
}

///|
fn StrictJsonScanner::scan_value(
  self : StrictJsonScanner,
  depth : Int,
) -> Unit raise {
  if depth > 64 {
    raise BridgeError("resource-exhausted: JSON recursion depth exceeds 64")
  }
  self.skip_whitespace()
  guard self.index < self.source.length() else {
    raise BridgeError("malformed-message: missing JSON value")
  }
  match self.source[self.index] {
    '{' => {
      self.index += 1
      self.skip_whitespace()
      if self.index < self.source.length() && self.source[self.index] == '}' {
        self.index += 1
        return
      }
      let members : Map[String, Bool] = {}
      let mut count = 0
      for ;; {
        let name = self.scan_string()
        if members.contains(name) {
          raise BridgeError("malformed-message: duplicate JSON object member")
        }
        members[name] = true
        count += 1
        if count > 100000 {
          raise BridgeError("resource-exhausted: JSON object exceeds 100000 members")
        }
        self.take(':')
        self.scan_value(depth + 1)
        self.skip_whitespace()
        guard self.index < self.source.length() else {
          raise BridgeError("malformed-message: unterminated JSON object")
        }
        if self.source[self.index] == '}' {
          self.index += 1
          return
        }
        self.take(',')
      }
    }
    '[' => {
      self.index += 1
      self.skip_whitespace()
      if self.index < self.source.length() && self.source[self.index] == ']' {
        self.index += 1
        return
      }
      let mut count = 0
      for ;; {
        self.scan_value(depth + 1)
        count += 1
        if count > 100000 {
          raise BridgeError("resource-exhausted: JSON array exceeds 100000 elements")
        }
        self.skip_whitespace()
        guard self.index < self.source.length() else {
          raise BridgeError("malformed-message: unterminated JSON array")
        }
        if self.source[self.index] == ']' {
          self.index += 1
          return
        }
        self.take(',')
      }
    }
    '"' => ignore(self.scan_string())
    _ => self.scan_primitive()
  }
}

///|
fn parse_strict_json(source : String) -> Json raise {
  let scanner = { source, index: 0 }
  scanner.scan_value(0)
  scanner.skip_whitespace()
  if scanner.index != source.length() {
    raise BridgeError("malformed-message: trailing JSON data")
  }
  @json.parse(source) catch {
    _ => raise BridgeError("malformed-message: invalid JSON")
  }
}
