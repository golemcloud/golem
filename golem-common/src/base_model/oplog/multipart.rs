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

/// A parsed part from a multipart/mixed message.
#[derive(Debug)]
pub struct MultipartPart<'a> {
    /// The `name` parameter from Content-Disposition, if present.
    pub name: Option<String>,
    /// The Content-Type header value, if present.
    pub content_type: Option<String>,
    /// The raw body bytes of this part.
    pub body: &'a [u8],
}

/// Extracts the boundary parameter from a `multipart/mixed; boundary=...` content-type string.
pub fn extract_boundary(content_type: &str) -> Option<&str> {
    let ct = content_type.strip_prefix("multipart/mixed")?;
    let rest = ct.trim_start();
    let rest = rest.strip_prefix(';')?;
    let rest = rest.trim_start();
    for param in rest.split(';') {
        let param = param.trim();
        if let Some(value) = param.strip_prefix("boundary=") {
            let value = value.trim();
            // Remove optional quotes
            if value.starts_with('"') && value.ends_with('"') && value.len() >= 2 {
                return Some(&value[1..value.len() - 1]);
            }
            return Some(value);
        }
    }
    None
}

/// Parses a multipart/mixed body into its constituent parts.
///
/// The `boundary` should be extracted from the Content-Type header using `extract_boundary`.
/// Returns `None` if the body cannot be parsed.
pub fn parse_multipart_mixed<'a>(boundary: &str, data: &'a [u8]) -> Option<Vec<MultipartPart<'a>>> {
    let delimiter = format!("--{boundary}");
    let close_delimiter = format!("--{boundary}--");

    let delimiter_bytes = delimiter.as_bytes();
    let close_delimiter_bytes = close_delimiter.as_bytes();

    let mut parts = Vec::new();
    let mut pos = 0;

    // Skip preamble: find the first delimiter
    pos = find_bytes(data, pos, delimiter_bytes)?;
    pos += delimiter_bytes.len();

    // Skip CRLF or LF after delimiter
    pos = skip_line_ending(data, pos);

    loop {
        // Check if we've hit the close delimiter
        if data[pos..].starts_with(close_delimiter_bytes) {
            break;
        }

        // Find the end of headers (empty line)
        let header_end = find_empty_line(data, pos)?;
        let headers_slice = &data[pos..header_end];
        let headers_str = std::str::from_utf8(headers_slice).ok()?;

        // Parse headers
        let mut name = None;
        let mut content_type = None;
        for line in headers_str.lines() {
            let line = line.trim();
            if let Some(ct) = line.strip_prefix("Content-Type:") {
                content_type = Some(ct.trim().to_string());
            } else if let Some(cd) = line.strip_prefix("Content-Disposition:") {
                // Extract name="..." parameter
                if let Some(name_start) = cd.find("name=\"") {
                    let rest = &cd[name_start + 6..];
                    if let Some(name_end) = rest.find('"') {
                        name = Some(rest[..name_end].to_string());
                    }
                }
            }
        }

        // Body starts after the empty line
        let body_start = skip_empty_line(data, header_end);

        // Find the next delimiter
        let body_end = find_bytes(data, body_start, delimiter_bytes)?;

        // Strip trailing CRLF or LF before the delimiter
        let body_end = strip_trailing_line_ending(data, body_start, body_end);

        parts.push(MultipartPart {
            name,
            content_type,
            body: &data[body_start..body_end],
        });

        // Move past the delimiter
        pos = body_end;
        // Skip the line ending before the delimiter
        pos = skip_line_ending(data, pos);
        pos = find_bytes(data, pos, delimiter_bytes)?;
        pos += delimiter_bytes.len();
        // Check for close delimiter suffix
        if data.get(pos..pos + 2) == Some(b"--") {
            break;
        }
        pos = skip_line_ending(data, pos);
    }

    Some(parts)
}

fn find_bytes(haystack: &[u8], start: usize, needle: &[u8]) -> Option<usize> {
    haystack[start..]
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|p| p + start)
}

fn skip_line_ending(data: &[u8], pos: usize) -> usize {
    if data.get(pos..pos + 2) == Some(b"\r\n") {
        pos + 2
    } else if data.get(pos) == Some(&b'\n') {
        pos + 1
    } else {
        pos
    }
}

fn find_empty_line(data: &[u8], start: usize) -> Option<usize> {
    // Look for \r\n\r\n or \n\n
    for i in start..data.len().saturating_sub(1) {
        if data[i] == b'\n' && data[i + 1] == b'\n' {
            return Some(i);
        }
        if i + 3 < data.len()
            && data[i] == b'\r'
            && data[i + 1] == b'\n'
            && data[i + 2] == b'\r'
            && data[i + 3] == b'\n'
        {
            return Some(i);
        }
    }
    None
}

fn skip_empty_line(data: &[u8], pos: usize) -> usize {
    if data.get(pos..pos + 4) == Some(b"\r\n\r\n") {
        pos + 4
    } else if data.get(pos..pos + 2) == Some(b"\n\n") {
        pos + 2
    } else {
        pos
    }
}

fn strip_trailing_line_ending(data: &[u8], body_start: usize, body_end: usize) -> usize {
    if body_end >= body_start + 2 && data[body_end - 2] == b'\r' && data[body_end - 1] == b'\n' {
        body_end - 2
    } else if body_end > body_start && data[body_end - 1] == b'\n' {
        body_end - 1
    } else {
        body_end
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    #[test]
    fn test_extract_boundary() {
        assert_eq!(
            extract_boundary("multipart/mixed; boundary=abc123"),
            Some("abc123")
        );
        assert_eq!(
            extract_boundary("multipart/mixed; boundary=\"abc-123\""),
            Some("abc-123")
        );
        assert_eq!(extract_boundary("application/json"), None);
    }

    #[test]
    fn test_parse_simple_multipart() {
        let boundary = "test-boundary";
        let body = format!(
            "--{boundary}\r\n\
             Content-Type: application/json\r\n\
             Content-Disposition: attachment; name=\"state\"\r\n\
             \r\n\
             {{\"key\":\"value\"}}\r\n\
             --{boundary}\r\n\
             Content-Type: application/x-sqlite3\r\n\
             Content-Disposition: attachment; name=\"db:main\"\r\n\
             \r\n\
             SQLITEDATA\r\n\
             --{boundary}--\r\n"
        );

        let parts = parse_multipart_mixed(boundary, body.as_bytes()).unwrap();
        assert_eq!(parts.len(), 2);

        assert_eq!(parts[0].name.as_deref(), Some("state"));
        assert_eq!(parts[0].content_type.as_deref(), Some("application/json"));
        assert_eq!(parts[0].body, b"{\"key\":\"value\"}");

        assert_eq!(parts[1].name.as_deref(), Some("db:main"));
        assert_eq!(
            parts[1].content_type.as_deref(),
            Some("application/x-sqlite3")
        );
        assert_eq!(parts[1].body, b"SQLITEDATA");
    }

    #[test]
    fn test_parse_multipart_with_binary() {
        let boundary = "uuid-boundary-12345";
        let json_part = b"{\"version\":1}";
        let binary_part: &[u8] = &[0x00, 0xFF, 0x53, 0x51, 0x4C];

        let mut body = Vec::new();
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        body.extend_from_slice(b"Content-Type: application/json\r\n");
        body.extend_from_slice(b"Content-Disposition: attachment; name=\"state\"\r\n");
        body.extend_from_slice(b"\r\n");
        body.extend_from_slice(json_part);
        body.extend_from_slice(format!("\r\n--{boundary}\r\n").as_bytes());
        body.extend_from_slice(b"Content-Type: application/x-sqlite3\r\n");
        body.extend_from_slice(b"Content-Disposition: attachment; name=\"db:cache\"\r\n");
        body.extend_from_slice(b"\r\n");
        body.extend_from_slice(binary_part);
        body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());

        let parts = parse_multipart_mixed(boundary, &body).unwrap();
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].name.as_deref(), Some("state"));
        assert_eq!(parts[0].body, json_part);
        assert_eq!(parts[1].name.as_deref(), Some("db:cache"));
        assert_eq!(parts[1].body, binary_part);
    }
}
