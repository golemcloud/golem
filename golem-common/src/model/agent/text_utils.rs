// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::fmt::Write;

/// Write a JSON-escaped version of `s` into `buf`.
pub fn write_json_escaped(buf: &mut String, s: &str) {
    for ch in s.chars() {
        write_json_escaped_char(buf, ch);
    }
}

/// Write a single JSON-escaped character into `buf`.
pub fn write_json_escaped_char(buf: &mut String, ch: char) {
    match ch {
        '"' => buf.push_str("\\\""),
        '\\' => buf.push_str("\\\\"),
        '\n' => buf.push_str("\\n"),
        '\r' => buf.push_str("\\r"),
        '\t' => buf.push_str("\\t"),
        '\u{08}' => buf.push_str("\\b"),
        '\u{0C}' => buf.push_str("\\f"),
        c if c.is_control() => {
            let mut code_units = [0u16; 2];
            let encoded = c.encode_utf16(&mut code_units);
            for unit in encoded {
                write!(buf, "\\u{:04x}", unit).unwrap();
            }
        }
        c => buf.push(c),
    }
}

/// Writes a formatted float string to `buf`, ensuring it contains a decimal point.
/// If it has an exponent but no dot (e.g., "1e20"), inserts ".0" before the exponent.
/// If it has neither dot nor exponent, appends ".0".
pub fn write_with_decimal_point(buf: &mut String, s: &str) {
    if s.contains('.') {
        buf.push_str(s);
    } else if let Some(e_pos) = s.find('e').or_else(|| s.find('E')) {
        buf.push_str(&s[..e_pos]);
        buf.push_str(".0");
        buf.push_str(&s[e_pos..]);
    } else {
        buf.push_str(s);
        buf.push_str(".0");
    }
}
