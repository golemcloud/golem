// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1 (https://license.golem.cloud/LICENSE).

//! Shared HTTP path and file-mapping grammar for metadata producers and consumers.

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum FileMapping {
    Exact {
        public_path: Vec<String>,
        file_path: String,
    },
    Subtree {
        public_prefix: Vec<String>,
        filesystem_root: String,
    },
}

/// A public request target with one shared segment boundary for routing and files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpRequestTarget {
    path: String,
    query: Option<String>,
    segments: Vec<String>,
    trailing_slash: bool,
}

impl HttpRequestTarget {
    pub fn parse(target: &str) -> Result<Self, String> {
        let (path, query) = match target.split_once('?') {
            Some((path, query)) => (path, Some(query.to_string())),
            None => (target, None),
        };
        let remaining = path.strip_prefix('/').ok_or("unsafe-path")?;
        let trailing_slash = !remaining.is_empty() && remaining.ends_with('/');
        let segments = if remaining.is_empty() {
            Vec::new()
        } else {
            let remaining = if trailing_slash {
                &remaining[..remaining.len() - 1]
            } else {
                remaining
            };
            remaining
                .split('/')
                .map(decode_segment)
                .collect::<Result<Vec<_>, _>>()?
        };
        Ok(Self {
            path: path.to_string(),
            query,
            segments,
            trailing_slash,
        })
    }

    pub fn path(&self) -> &str {
        &self.path
    }
    pub fn query(&self) -> Option<&str> {
        self.query.as_deref()
    }
    pub fn segments(&self) -> &[String] {
        &self.segments
    }
    pub fn trailing_slash(&self) -> bool {
        self.trailing_slash
    }
}

impl FileMapping {
    /// Compile URI source syntax once; targets are filesystem text, not URIs.
    pub fn compile(source: &str, target: &str) -> Result<Self, String> {
        let (source, subtree) = match source.strip_suffix("/*") {
            Some(prefix) if prefix.ends_with('/') => return Err("source-path".into()),
            Some(prefix) => (if prefix.is_empty() { "/" } else { prefix }, true),
            None => (source, false),
        };
        if source.contains('*') {
            return Err("source-wildcard".into());
        }
        if source.contains(['$', '?', '#']) {
            return Err("source-path".into());
        }
        let path = HttpRequestTarget::parse(source).map_err(|_| "source-path")?;
        if path.trailing_slash {
            return Err("source-path".into());
        }
        let (target, allow_root) = if subtree {
            let root = target.strip_suffix("/$1").ok_or("target-placeholder")?;
            if root.ends_with('/') {
                return Err("target-path".into());
            }
            (if root.is_empty() { "/" } else { root }, true)
        } else {
            (target, false)
        };
        validate_file_mapping(&path.segments, target, allow_root)?;
        Ok(if subtree {
            Self::Subtree {
                public_prefix: path.segments,
                filesystem_root: target.into(),
            }
        } else {
            Self::Exact {
                public_path: path.segments,
                file_path: target.into(),
            }
        })
    }
}

/// Validate already-decoded structural mapping fields without decoding again.
pub fn validate_file_mapping(
    segments: &[String],
    target: &str,
    allow_root: bool,
) -> Result<(), String> {
    if segments
        .iter()
        .any(|segment| !valid_decoded_segment(segment))
    {
        return Err("source-path".into());
    }
    if target.contains('$') {
        return Err("target-placeholder".into());
    }
    if !target.starts_with('/')
        || target.chars().any(char::is_control)
        || (!allow_root && target == "/")
        || (target != "/"
            && target[1..]
                .split('/')
                .any(|segment| !valid_decoded_segment(segment)))
    {
        return Err("target-path".into());
    }
    Ok(())
}

pub fn valid_decoded_segment(segment: &str) -> bool {
    !segment.is_empty()
        && segment != "."
        && segment != ".."
        && !segment
            .bytes()
            .any(|byte| byte <= 0x1f || byte == 0x7f || byte == b'/' || byte == b'\\')
}

fn decode_segment(raw: &str) -> Result<String, String> {
    let mut decoded = Vec::with_capacity(raw.len());
    let mut bytes = raw.bytes();
    while let Some(byte) = bytes.next() {
        if byte == b'%' {
            let high = bytes.next().and_then(hex).ok_or("unsafe-path")?;
            let low = bytes.next().and_then(hex).ok_or("unsafe-path")?;
            decoded.push(high * 16 + low);
        } else if byte.is_ascii_alphanumeric() || b"-._~!$&'()*+,;=:@".contains(&byte) {
            decoded.push(byte);
        } else {
            return Err("unsafe-path".into());
        }
    }
    let decoded = String::from_utf8(decoded).map_err(|_| "unsafe-path")?;
    if !valid_decoded_segment(&decoded) {
        return Err("unsafe-path".into());
    }
    Ok(decoded)
}

fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}
