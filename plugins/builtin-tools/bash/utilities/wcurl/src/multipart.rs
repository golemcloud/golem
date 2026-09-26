//! `-F`/`--form`: builds a `multipart/form-data` body from parsed [`FormField`](crate::parse::FormField)s.
//!
//! The boundary is a FIXED constant rather than curl's randomly generated one — this crate has no
//! dependency on a CSPRNG (deliberately: wasm32-wasip2 randomness is available but every other
//! part of this crate avoids depending on it so its output stays reproducible for tests and
//! replay), and a fixed boundary is what lets a unit test assert on the exact body bytes. The
//! tradeoff is the classic multipart one: a field whose CONTENT happens to contain a line matching
//! the boundary would corrupt the body — real curl has the same failure mode with low enough
//! probability from randomness that it's ignored in practice; a fixed boundary makes the
//! probability the SAME every time rather than vanishingly small, which is a real (if narrow)
//! regression curl doesn't have. Accepted for this scope.

use crate::parse::FormField;

/// The fixed multipart boundary every `-F` request uses. 48 hex characters is enough that a body
/// line colliding with it by accident is not a realistic concern for ordinary form data.
pub(crate) const BOUNDARY: &str = "----bashtoolWcurlBoundary7f3a2c19d4e8b6015c9a3f7e2b1d4c8a";

/// Build the `multipart/form-data` body and its `Content-Type` header value for `fields`, reading
/// any `@path` field from disk (resolved against `cwd`).
pub(crate) fn build(
    fields: &[FormField],
    cwd: &std::path::Path,
) -> Result<(Vec<u8>, String), String> {
    let mut body = Vec::new();
    for field in fields {
        body.extend_from_slice(b"--");
        body.extend_from_slice(BOUNDARY.as_bytes());
        body.extend_from_slice(b"\r\n");
        match field {
            FormField::Text { name, value } => {
                write_header(&mut body, name, None, None);
                body.extend_from_slice(value.as_bytes());
            }
            FormField::File {
                name,
                path,
                content_type,
            } => {
                let filename = path.rsplit(['/', '\\']).next().unwrap_or(path);
                let content_type = content_type
                    .clone()
                    .unwrap_or_else(|| guess_content_type(filename));
                let bytes = std::fs::read(cwd.join(path))
                    .map_err(|e| format!("-F: cannot read '{path}': {e}"))?;
                write_header(&mut body, name, Some(filename), Some(&content_type));
                body.extend_from_slice(&bytes);
            }
        }
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(b"--");
    body.extend_from_slice(BOUNDARY.as_bytes());
    body.extend_from_slice(b"--\r\n");
    Ok((body, format!("multipart/form-data; boundary={BOUNDARY}")))
}

fn write_header(out: &mut Vec<u8>, name: &str, filename: Option<&str>, content_type: Option<&str>) {
    out.extend_from_slice(b"Content-Disposition: form-data; name=\"");
    out.extend_from_slice(escape_quoted(name).as_bytes());
    out.push(b'"');
    if let Some(filename) = filename {
        out.extend_from_slice(b"; filename=\"");
        out.extend_from_slice(escape_quoted(filename).as_bytes());
        out.push(b'"');
    }
    out.extend_from_slice(b"\r\n");
    if let Some(content_type) = content_type {
        out.extend_from_slice(b"Content-Type: ");
        out.extend_from_slice(content_type.as_bytes());
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(b"\r\n");
}

/// Escape `"` and `\` in a quoted-string header parameter (RFC 7578 §4.2 / RFC 2183).
fn escape_quoted(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

/// A tiny, dependency-free content-type guess from a filename extension — good enough for the
/// common cases `-F name=@file` is used for; anything else falls back to the generic octet type,
/// same as curl does when it can't guess either.
fn guess_content_type(filename: &str) -> String {
    let ext = filename
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "txt" => "text/plain",
        "html" | "htm" => "text/html",
        "json" => "application/json",
        "xml" => "application/xml",
        "csv" => "text/csv",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "pdf" => "application/pdf",
        _ => "application/octet-stream",
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_field_is_a_simple_disposition_part() {
        let (body, content_type) = build(
            &[FormField::Text {
                name: "a".into(),
                value: "1".into(),
            }],
            std::path::Path::new("."),
        )
        .unwrap();
        let text = String::from_utf8(body).unwrap();
        assert!(content_type.starts_with("multipart/form-data; boundary="));
        assert!(text.contains("Content-Disposition: form-data; name=\"a\"\r\n\r\n1"));
        assert!(text.starts_with(&format!("--{BOUNDARY}\r\n")));
        assert!(text.ends_with(&format!("--{BOUNDARY}--\r\n")));
    }

    #[test]
    fn file_field_reads_the_file_and_guesses_the_content_type() {
        let dir = std::env::temp_dir().join(format!("wcurl_multipart_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("data.json");
        std::fs::write(&path, b"{\"x\":1}").unwrap();
        let (body, _) = build(
            &[FormField::File {
                name: "upload".into(),
                path: "data.json".into(),
                content_type: None,
            }],
            &dir,
        )
        .unwrap();
        let text = String::from_utf8(body).unwrap();
        assert!(text.contains("name=\"upload\"; filename=\"data.json\""));
        assert!(text.contains("Content-Type: application/json"));
        assert!(text.contains("{\"x\":1}"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn explicit_content_type_overrides_the_guess() {
        let dir = std::env::temp_dir().join(format!("wcurl_multipart_ct_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("blob");
        std::fs::write(&path, b"raw").unwrap();
        let (body, _) = build(
            &[FormField::File {
                name: "f".into(),
                path: "blob".into(),
                content_type: Some("application/x-custom".into()),
            }],
            &dir,
        )
        .unwrap();
        assert!(
            String::from_utf8(body)
                .unwrap()
                .contains("Content-Type: application/x-custom")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_file_is_an_error() {
        let result = build(
            &[FormField::File {
                name: "f".into(),
                path: "does-not-exist".into(),
                content_type: None,
            }],
            std::path::Path::new("."),
        );
        assert!(result.is_err());
    }

    #[test]
    fn multiple_fields_preserve_order() {
        let (body, _) = build(
            &[
                FormField::Text {
                    name: "first".into(),
                    value: "1".into(),
                },
                FormField::Text {
                    name: "second".into(),
                    value: "2".into(),
                },
            ],
            std::path::Path::new("."),
        )
        .unwrap();
        let text = String::from_utf8(body).unwrap();
        assert!(text.find("first").unwrap() < text.find("second").unwrap());
    }
}
