use serde::{Serialize, de::DeserializeOwned};
use std::collections::{BTreeMap, HashSet};

/// Opaque application-owned bytes with a bare MIME type. Names are not paths.
#[derive(Debug, Clone, PartialEq)]
pub struct SnapshotPart {
    pub bytes: Vec<u8>,
    pub content_type: String,
}

/// JSON-backed state projected separately from resource-bearing live state.
/// Binary-only snapshots use `serde_json::Value::Null`. Hooks run unpersisted
/// and must tolerate retries. Whole-buffer copies amplify memory use; the host
/// inline/blob threshold is not a snapshot size cap.
#[derive(Debug, Clone, PartialEq)]
pub struct MultipartSnapshot {
    pub state: serde_json::Value,
    pub parts: BTreeMap<String, SnapshotPart>,
}

impl TryFrom<MultipartSnapshot> for super::SnapshotData {
    type Error = String;

    fn try_from(snapshot: MultipartSnapshot) -> Result<Self, Self::Error> {
        Ok(Self::Multipart {
            state: serde_json::to_vec(&snapshot.state).map_err(|e| e.to_string())?,
            parts: snapshot.parts,
        })
    }
}

impl TryFrom<super::SnapshotData> for MultipartSnapshot {
    type Error = String;

    fn try_from(snapshot: super::SnapshotData) -> Result<Self, Self::Error> {
        let super::SnapshotData::Multipart { state, parts } = snapshot else {
            return Err("expected multipart snapshot".to_string());
        };
        Ok(Self {
            state: serde_json::from_slice(&state).map_err(|e| e.to_string())?,
            parts,
        })
    }
}

impl MultipartSnapshot {
    pub fn from_state<S: Serialize>(
        state: &S,
        parts: BTreeMap<String, SnapshotPart>,
    ) -> Result<Self, String> {
        Ok(Self {
            state: serde_json::to_value(state).map_err(|e| e.to_string())?,
            parts,
        })
    }

    pub fn decode_state<S: DeserializeOwned>(&self) -> Result<S, String> {
        serde_json::from_value(self.state.clone()).map_err(|e| e.to_string())
    }

    pub fn require_part(&self, name: &str, expected_content_type: &str) -> Result<&[u8], String> {
        let part = self
            .parts
            .get(name)
            .ok_or_else(|| format!("missing required part '{name}'"))?;
        if normalize_content_type(&part.content_type)?
            != normalize_content_type(expected_content_type)?
        {
            return Err(format!(
                "part '{name}' has unexpected Content-Type '{}'",
                part.content_type
            ));
        }
        Ok(&part.bytes)
    }
}

fn valid_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    bytes
        .next()
        .is_some_and(|b| b.is_ascii_alphanumeric() || b == b'_')
        && bytes.all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
}

fn normalize_content_type(value: &str) -> Result<String, String> {
    let valid_token = |s: &str| {
        !s.is_empty()
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+.^_`|~-".contains(&b))
    };
    if !value
        .split_once('/')
        .is_some_and(|(ty, subtype)| valid_token(ty) && valid_token(subtype))
    {
        return Err(format!("invalid snapshot Content-Type '{value}'"));
    }
    Ok(value.to_ascii_lowercase())
}

pub(super) fn extract_boundary(mime: &str) -> Option<&str> {
    let mut params = mime.split(';');
    if !params
        .next()?
        .trim()
        .eq_ignore_ascii_case("multipart/mixed")
    {
        return None;
    }
    let (key, value) = params.next()?.split_once('=')?;
    if params.next().is_some() || !key.trim().eq_ignore_ascii_case("boundary") {
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

fn valid_boundary(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 70
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"'()+_,./:=?-".contains(&b))
}

fn delimiter(data: &[u8], marker: &[u8], newline: &[u8], pos: usize) -> Option<(usize, bool)> {
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

pub(super) struct WirePart<'a> {
    pub name: String,
    pub content_type: String,
    pub body: &'a [u8],
}

pub(super) fn parse_parts<'a>(boundary: &str, data: &'a [u8]) -> Option<Vec<WirePart<'a>>> {
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
    let mut suffix = start + marker.len();
    if data.get(suffix..suffix + 2) == Some(b"--") {
        suffix += 2;
    }
    let newline: &[u8] =
        if data.get(suffix..suffix + 2) == Some(b"\r\n") || (suffix == data.len() && start == 2) {
            b"\r\n"
        } else {
            b"\n"
        };
    if start != 0 && start != newline.len() {
        return None;
    }
    let (mut pos, mut closing) = delimiter(data, marker, newline, start)?;
    let mut parts = Vec::new();
    let mut names = HashSet::new();
    while !closing {
        let mut name = None;
        let mut content_type = None;
        let mut headers = HashSet::new();
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
            let (key, value) = std::str::from_utf8(line).ok()?.split_once(':')?;
            let key = key.to_ascii_lowercase();
            if !headers.insert(key.clone()) {
                return None;
            }
            let value = value.trim();
            match key.as_str() {
                "content-type" => content_type = Some(value.to_string()),
                "content-disposition" => {
                    let (kind, param) = value.split_once(';')?;
                    if !kind.eq_ignore_ascii_case("attachment") {
                        return None;
                    }
                    let (key, value) = param.trim().split_once('=')?;
                    if !key.eq_ignore_ascii_case("name") {
                        return None;
                    }
                    let value = value.strip_prefix('"')?.strip_suffix('"')?;
                    if value.is_empty() || value.contains(['"', '\\']) {
                        return None;
                    }
                    name = Some(value.to_string());
                }
                _ => return None,
            }
        }
        let name = name?;
        if !names.insert(name.clone()) {
            return None;
        }
        let content_type = content_type?;
        let (body_end, next, close) = (pos..data.len()).find_map(|i| {
            data[i..]
                .starts_with(newline)
                .then(|| delimiter(data, marker, newline, i + newline.len()))
                .flatten()
                .map(|(end, close)| (i, end, close))
        })?;
        parts.push(WirePart {
            name,
            content_type,
            body: &data[pos..body_end],
        });
        pos = next;
        closing = close;
    }
    (pos == data.len()).then_some(parts)
}

pub(super) fn encode(
    state: &[u8],
    user_parts: &BTreeMap<String, SnapshotPart>,
    principal: &super::Principal,
) -> Result<(Vec<u8>, String), String> {
    #[derive(serde::Serialize)]
    struct Envelope<'a> {
        version: u8,
        principal: &'a super::Principal,
        state: &'a serde_json::value::RawValue,
    }
    let state = serde_json::from_slice(state).map_err(|e| e.to_string())?;
    let state = serde_json::to_vec(&Envelope {
        version: 1,
        principal,
        state,
    })
    .map_err(|e| e.to_string())?;
    let mut parts = vec![(
        "state".to_string(),
        "application/json".to_string(),
        state.as_slice(),
    )];
    for (name, part) in user_parts {
        if !valid_name(name) {
            return Err(format!("invalid snapshot part name '{name}'"));
        }
        parts.push((
            format!("part:{name}"),
            normalize_content_type(&part.content_type)?,
            part.bytes.as_slice(),
        ));
    }
    let mut nonce = 0;
    let boundary = loop {
        let candidate = format!("golem-snapshot-{nonce}");
        let marker = format!("--{candidate}");
        let collision = parts.iter().any(|(_, _, body)| {
            let mut framed = body.to_vec();
            framed.extend_from_slice(b"\r\n");
            delimiter(&framed, marker.as_bytes(), b"\r\n", 0).is_some()
                || (0..framed.len()).any(|i| {
                    framed[i..].starts_with(b"\r\n")
                        && delimiter(&framed, marker.as_bytes(), b"\r\n", i + 2).is_some()
                })
        });
        if !collision {
            break candidate;
        }
        nonce += 1;
    };
    let mut data = Vec::new();
    for (name, content_type, body) in parts {
        data.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: attachment; name=\"{name}\"\r\nContent-Type: {content_type}\r\n\r\n").as_bytes());
        data.extend_from_slice(body);
        data.extend_from_slice(b"\r\n");
    }
    data.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    Ok((data, format!("multipart/mixed; boundary={boundary}")))
}

pub(super) fn decode(
    data: &[u8],
    mime: &str,
) -> Result<(super::Principal, super::SnapshotData), String> {
    let invalid = || "invalid multipart snapshot".to_string();
    let boundary = extract_boundary(mime).ok_or_else(invalid)?;
    let parts = parse_parts(boundary, data).ok_or_else(invalid)?;
    let state = parts
        .iter()
        .find(|p| p.name == "state")
        .ok_or_else(invalid)?;
    if state.content_type != "application/json" {
        return Err(invalid());
    }
    #[derive(serde::Deserialize)]
    struct Envelope<'a> {
        #[serde(borrow)]
        version: &'a serde_json::value::RawValue,
        #[serde(borrow)]
        principal: &'a serde_json::value::RawValue,
        #[serde(borrow)]
        state: &'a serde_json::value::RawValue,
    }
    if state
        .body
        .iter()
        .copied()
        .find(|b| !b.is_ascii_whitespace())
        != Some(b'{')
    {
        return Err("multipart envelope must be an object".to_string());
    }
    let envelope: Envelope<'_> = serde_json::from_slice(state.body).map_err(|e| e.to_string())?;
    if envelope.version.get() != "1" {
        return Err("multipart version must be integer 1".to_string());
    }
    if !envelope.principal.get().starts_with('{') {
        return Err("multipart principal must be an object".to_string());
    }
    #[derive(serde::Deserialize)]
    struct PrincipalRecord<'a> {
        tag: String,
        #[serde(borrow)]
        val: Option<&'a serde_json::value::RawValue>,
    }
    let record: PrincipalRecord<'_> =
        serde_json::from_str(envelope.principal.get()).map_err(|e| e.to_string())?;
    if record.tag != "anonymous" && !record.val.is_some_and(|value| value.get().starts_with('{')) {
        return Err("multipart principal payload must be an object".to_string());
    }
    let principal = super::principal_serde::from_json_bytes(envelope.principal.get().as_bytes())
        .map_err(|e| e.to_string())?;
    let state = envelope.state.get().as_bytes().to_vec();
    let mut user_parts = BTreeMap::new();
    for part in parts {
        if part.name == "state" {
            continue;
        }
        let name = part
            .name
            .strip_prefix("part:")
            .filter(|name| valid_name(name))
            .ok_or_else(invalid)?;
        user_parts.insert(
            name.to_string(),
            SnapshotPart {
                bytes: part.body.to_vec(),
                content_type: normalize_content_type(&part.content_type)?,
            },
        );
    }
    Ok((
        principal,
        super::SnapshotData::Multipart {
            state,
            parts: user_parts,
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    fn encode(
        snapshot: &MultipartSnapshot,
        principal: &super::super::Principal,
    ) -> Result<(Vec<u8>, String), String> {
        super::encode(
            &serde_json::to_vec(&snapshot.state).unwrap(),
            &snapshot.parts,
            principal,
        )
    }

    fn decode(
        data: &[u8],
        mime: &str,
    ) -> Result<(super::super::Principal, MultipartSnapshot), String> {
        let (principal, transport) = super::decode(data, mime)?;
        Ok((principal, transport.try_into()?))
    }

    #[test]
    fn multipart_transport_embeds_raw_state_once_and_defers_ast_to_adapter() {
        let state = br#"{ "revision" : 1e0, "label" : "\u0061" }"#;
        let (data, mime) =
            super::encode(state, &BTreeMap::new(), &super::super::Principal::Anonymous).unwrap();
        let parts = parse_parts(extract_boundary(&mime).unwrap(), &data).unwrap();
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].body, br#"{"version":1,"principal":{"tag":"anonymous"},"state":{ "revision" : 1e0, "label" : "\u0061" }}"#);
        let (
            _,
            super::super::SnapshotData::Multipart {
                state: restored,
                parts,
            },
        ) = super::decode(&data, &mime).unwrap()
        else {
            panic!("multipart transport expected")
        };
        assert_eq!(restored, state);
        assert!(parts.is_empty());
        for state in [b"1e400".as_slice(), br#""\uD800""#] {
            let (data, mime) =
                super::encode(state, &BTreeMap::new(), &super::super::Principal::Anonymous)
                    .unwrap();
            let (_, transport) = super::decode(&data, &mime).unwrap();
            assert!(MultipartSnapshot::try_from(transport).is_err());
        }
    }

    #[test]
    fn multipart_carrier_roundtrip_preserves_dynamic_opaque_parts() {
        let mut parts = BTreeMap::new();
        for (name, bytes) in [
            ("index", (0..=255).collect()),
            ("__proto__", vec![]),
            ("state", vec![0, 255, 13, 10]),
        ] {
            parts.insert(
                name.to_string(),
                SnapshotPart {
                    bytes,
                    content_type: "Application/Octet-Stream".to_string(),
                },
            );
        }
        let snapshot = MultipartSnapshot::from_state(
            &serde_json::json!({"revision":17,"text":"Árvíz 🦀"}),
            parts,
        )
        .unwrap();
        let (encoded, mime) = encode(&snapshot, &super::super::Principal::Anonymous).unwrap();
        let (principal, decoded) = decode(&encoded, &mime).unwrap();
        assert!(matches!(principal, super::super::Principal::Anonymous));
        assert_eq!(decoded.state, snapshot.state);
        assert_eq!(decoded.parts.len(), 3);
        for (name, part) in snapshot.parts {
            assert_eq!(
                decoded
                    .require_part(&name, "APPLICATION/OCTET-STREAM")
                    .unwrap(),
                part.bytes
            );
        }
        assert!(decoded.require_part("missing", "text/plain").is_err());
        assert!(decoded.require_part("index", "text/plain").is_err());
        assert!(
            decoded
                .require_part("index", "application/octet-stream; charset=utf-8")
                .is_err()
        );
        let empty = MultipartSnapshot::from_state(&(), BTreeMap::new()).unwrap();
        let (data, mime) = encode(&empty, &super::super::Principal::Anonymous).unwrap();
        assert_eq!(decode(&data, &mime).unwrap().1, empty);
    }

    #[test]
    fn multipart_shared_framing_fixtures_and_collisions() {
        let fixtures: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../../test-data/snapshot-multipart/framing.json"
        ))
        .unwrap();
        let boundary = fixtures["boundary"].as_str().unwrap();
        for fixture in fixtures["valid"].as_array().unwrap() {
            let newline = fixture["newline"].as_str().unwrap();
            let body = fixture["payload"].as_str().unwrap();
            let data = format!(
                "--{boundary}{newline}Content-Disposition: attachment; name=\"part:opaque\"{newline}Content-Type: application/octet-stream{newline}{newline}{body}{newline}--{boundary}--{newline}"
            );
            let parts = parse_parts(boundary, data.as_bytes()).unwrap();
            assert_eq!(parts.len(), 1);
            assert_eq!(parts[0].body, body.as_bytes(), "{}", fixture["name"]);
        }
        let bytes = b"--golem-snapshot-0\r\npayload\r\n--golem-snapshot-1--".to_vec();
        let snapshot = MultipartSnapshot {
            state: serde_json::Value::Null,
            parts: BTreeMap::from([(
                "index".to_string(),
                SnapshotPart {
                    bytes: bytes.clone(),
                    content_type: "application/octet-stream".to_string(),
                },
            )]),
        };
        let (data, mime) = encode(&snapshot, &super::super::Principal::Anonymous).unwrap();
        assert_eq!(extract_boundary(&mime), Some("golem-snapshot-2"));
        assert_eq!(
            decode(&data, &mime)
                .unwrap()
                .1
                .require_part("index", "application/octet-stream")
                .unwrap(),
            bytes
        );
    }

    #[test]
    fn multipart_rejects_malformed_framing_metadata_names_and_namespaces() {
        let envelope = r#"{"version":1,"principal":{"tag":"anonymous"},"state":null}"#;
        let wrap = |state: &str, extra: &str| {
            format!(
                "--b\r\nContent-Type: application/json\r\nContent-Disposition: attachment; name=\"state\"\r\n\r\n{state}\r\n{extra}--b--\r\n"
            )
        };
        let valid = wrap(envelope, "");
        assert!(decode(valid.as_bytes(), "multipart/mixed; boundary=b").is_ok());
        assert!(
            decode(
                wrap(&envelope.replace("null", "[17,null]"), "").as_bytes(),
                "multipart/mixed; boundary=b"
            )
            .is_ok()
        );
        for state in [
            r#"[1,{"tag":"anonymous"},null]"#.to_string(),
            r#"{"version":1,"principal":["anonymous",null],"state":null}"#.to_string(),
            r#"{"version":1,"principal":{"tag":"agent","val":["10203040-5060-7080-9012-3456789abcde","worker(7)"]},"state":null}"#.to_string(),
            r#"{"version":1,"principal":{"tag":"golem-user","val":["fedcba98-7654-3210-9876-543210abcdef"]},"state":null}"#.to_string(),
            r#"{"version":1,"principal":{"tag":"oidc","val":["subject","issuer",null,null,null,null,null,null,null,"{}"]},"state":null}"#.to_string(),
            envelope.replace("\"version\":1", "\"version\":1,\"version\":1"),
            envelope.replace("\"version\":1", "\"version\":1.0"),
            envelope.replace("\"version\":1", "\"version\":1e0"),
            envelope.replace(
                "\"tag\":\"anonymous\"",
                "\"tag\":\"anonymous\",\"tag\":\"anonymous\"",
            ),
        ] {
            assert!(decode(wrap(&state, "").as_bytes(), "multipart/mixed; boundary=b").is_err());
        }
        for name in ["part:", "part:a/b", "db:main", "unknown:x"] {
            let extra = format!(
                "--b\r\nContent-Type: application/octet-stream\r\nContent-Disposition: attachment; name=\"{name}\"\r\n\r\nx\r\n"
            );
            assert!(
                decode(
                    wrap(envelope, &extra).as_bytes(),
                    "multipart/mixed; boundary=b"
                )
                .is_err()
            );
        }
        for data in [
            valid.replace("--b--\r\n", ""),
            valid.clone() + "epilogue",
            valid.replace(
                "Content-Type: application/json",
                "Content-Type: application/json\r\nContent-Type: application/json",
            ),
            valid.replace(
                "Content-Type: application/json",
                "Content-Type: application/json\r\nUnknown: x",
            ),
        ] {
            assert!(decode(data.as_bytes(), "multipart/mixed; boundary=b").is_err());
        }
        for (name, content_type) in [
            ("a/b", "text/plain"),
            ("index", "text/plain; charset=utf-8"),
            ("index", "text/plain\r\nHeader: injected"),
        ] {
            let snapshot = MultipartSnapshot {
                state: serde_json::Value::Null,
                parts: BTreeMap::from([(
                    name.to_string(),
                    SnapshotPart {
                        bytes: vec![],
                        content_type: content_type.to_string(),
                    },
                )]),
            };
            assert!(encode(&snapshot, &super::super::Principal::Anonymous).is_err());
        }
        let mut data = valid.into_bytes();
        let pos = data.windows(4).position(|w| w == b"null").unwrap();
        data[pos] = 255;
        assert!(decode(&data, "multipart/mixed; boundary=b").is_err());
    }
}
