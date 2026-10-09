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
    let mut sections = content_type.split(';');
    if !sections
        .next()?
        .trim()
        .eq_ignore_ascii_case("multipart/mixed")
    {
        return None;
    }
    let (key, value) = sections.next()?.split_once('=')?;
    if sections.next().is_some() || !key.trim().eq_ignore_ascii_case("boundary") {
        return None;
    }
    let value = value.trim();
    let value = if value.starts_with('"') {
        value.strip_prefix('"')?.strip_suffix('"')?
    } else {
        if !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"'+_.-".contains(&b))
        {
            return None;
        }
        value
    };
    valid_boundary(value).then_some(value)
}

/// Parses a multipart/mixed body into its constituent parts.
///
/// The `boundary` should be extracted from the Content-Type header using `extract_boundary`.
/// Returns `None` if the body cannot be parsed.
pub fn parse_multipart_mixed<'a>(boundary: &str, data: &'a [u8]) -> Option<Vec<MultipartPart<'a>>> {
    if !valid_boundary(boundary) {
        return None;
    }
    let marker = format!("--{boundary}");
    let marker = marker.as_bytes();
    let start = if data.starts_with(b"\r\n") {
        2
    } else if data.starts_with(b"\n") {
        1
    } else {
        0
    };
    let suffix = start + marker.len();
    let suffix = if data.get(suffix..suffix + 2) == Some(b"--") {
        suffix + 2
    } else {
        suffix
    };
    let newline: &[u8] =
        if data.get(suffix..suffix + 2) == Some(b"\r\n") || (suffix == data.len() && start == 2) {
            b"\r\n"
        } else {
            b"\n"
        };
    if start != 0 && start != newline.len() {
        return None;
    }
    let (mut pos, mut closing) = delimiter_at(data, marker, newline, start)?;
    let mut parts = Vec::new();
    let mut names = std::collections::HashSet::new();
    while !closing {
        let mut name = None;
        let mut content_type = None;
        let mut headers = std::collections::HashSet::new();
        loop {
            let end = data[pos..].iter().position(|b| *b == b'\n')? + pos;
            let line = data[pos..end]
                .strip_suffix(b"\r")
                .unwrap_or(&data[pos..end]);
            pos = end + 1;
            if line.is_empty() {
                break;
            }
            if !line.iter().all(|b| (32..=126).contains(b)) || line[0] == b' ' {
                return None;
            }
            let line = std::str::from_utf8(line).ok()?;
            let (key, value) = line.split_once(':')?;
            let key = key.to_ascii_lowercase();
            if !headers.insert(key.clone()) {
                return None;
            }
            let value = value.trim();
            match key.as_str() {
                "content-type" => content_type = Some(value.to_string()),
                "content-disposition" => {
                    let (disposition, parameter) = value.split_once(';')?;
                    if !disposition.eq_ignore_ascii_case("attachment") {
                        return None;
                    }
                    let (key, value) = parameter.trim().split_once('=')?;
                    if !key.eq_ignore_ascii_case("name") {
                        return None;
                    }
                    let value = value.strip_prefix('"')?.strip_suffix('"')?;
                    if value.is_empty() || value.contains(['"', '\\']) {
                        return None;
                    }
                    name = Some(value.to_string());
                }
                // The renderer is generic: no SDK namespace or state-envelope policy.
                _ => {}
            }
        }
        if let Some(name) = &name
            && !names.insert(name.clone())
        {
            return None;
        }
        let (body_end, next_pos, next_closing) = (pos..data.len()).find_map(|i| {
            if data[i..].starts_with(newline) {
                delimiter_at(data, marker, newline, i + newline.len())
                    .map(|(end, closing)| (i, end, closing))
            } else {
                None
            }
        })?;
        parts.push(MultipartPart {
            name,
            content_type,
            body: &data[pos..body_end],
        });
        pos = next_pos;
        closing = next_closing;
    }
    (pos == data.len()).then_some(parts)
}

fn valid_boundary(boundary: &str) -> bool {
    !boundary.is_empty()
        && boundary.len() <= 70
        && boundary
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"'()+_,./:=?-".contains(&b))
}

fn delimiter_at(data: &[u8], marker: &[u8], newline: &[u8], pos: usize) -> Option<(usize, bool)> {
    if !data.get(pos..)?.starts_with(marker) {
        return None;
    }
    let mut end = pos + marker.len();
    let closing = data[end..].starts_with(b"--");
    if closing {
        end += 2;
    }
    if data[end..].starts_with(newline) {
        Some((end + newline.len(), closing))
    } else if closing && end == data.len() {
        Some((end, closing))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    #[test]
    fn rejects_malformed_framing_and_duplicate_headers() {
        let header = "Content-Type: application/octet-stream\r\nContent-Disposition: attachment; name=\"part:index\"\r\n";
        let wire = format!("--b\r\n{header}\r\nX\r\n--b--\r\n");
        for bad in [
            wire[..wire.len() - 7].to_string(),
            format!("{wire}epilogue"),
            format!("{wire}\r\n"),
            format!("preamble{wire}"),
            format!("\r\n\r\n{wire}"),
            wire.replace("--b\r\n", "--bextra\r\n"),
            wire.replace("--b--\r\n", "--b--extra\r\n"),
            wire.replace(header, &format!("{header}content-type: text/plain\r\n")),
            wire.replace(
                header,
                &format!("{header}CONTENT-DISPOSITION: attachment; name=\"other\"\r\n"),
            ),
            wire.replace("name=\"part:index\"", "name=\"part:index\"; name=\"other\""),
            wire.replace("application/octet-stream", "application/\roctet-stream"),
        ] {
            assert!(
                parse_multipart_mixed("b", bad.as_bytes()).is_none(),
                "{bad:?}"
            );
        }
        assert_eq!(
            parse_multipart_mixed(
                "b",
                format!("\r\n{}", wire.trim_end_matches("\r\n")).as_bytes()
            )
            .unwrap()[0]
                .body,
            b"X"
        );
    }

    #[test]
    fn preserves_every_byte_and_generic_parts_without_sdk_metadata() {
        let expected: Vec<u8> = (0..=255).collect();
        let mut wire = b"--b\r\nX-Extra: generic\r\n\r\n".to_vec();
        wire.extend_from_slice(&expected);
        wire.extend_from_slice(b"\r\n--b--");
        let parts = parse_multipart_mixed("b", &wire).unwrap();
        assert_eq!(parts[0].body, expected);
        assert_eq!(parts[0].name, None);
        assert_eq!(parts[0].content_type, None);
    }

    #[test]
    fn shared_framing_preserves_payload_bytes() {
        let fixtures: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../test-data/snapshot-multipart/framing.json"
        ))
        .unwrap();
        let boundary = fixtures["boundary"].as_str().unwrap();
        for fixture in fixtures["valid"].as_array().unwrap() {
            let e = fixture["newline"].as_str().unwrap();
            let payload = fixture["payload"].as_str().unwrap();
            let wire = format!(
                "--{boundary}{e}Content-Type: application/octet-stream{e}Content-Disposition: attachment; name=\"part:index\"{e}{e}{payload}{e}--{boundary}--{e}"
            );
            let hex = fixture["hex"].as_str().unwrap();
            let expected: Vec<u8> = (0..hex.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
                .collect();
            let parts = parse_multipart_mixed(boundary, wire.as_bytes()).unwrap();
            assert_eq!(parts.len(), 1, "{}", fixture["name"]);
            assert_eq!(parts[0].body, expected, "{}", fixture["name"]);
        }
    }

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
