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

use crate::config::InvocationSessionTokenConfig;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use golem_common::model::invocation_session_public::{MAX_TOKEN_SIZE, PublicErrorCode};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::collections::{BTreeMap, HashMap};
use std::fmt::{Display, Formatter};
use uuid::Uuid;

const TOKEN_PREFIX: &str = "gai1_";
const FORMAT_VERSION: u8 = 1;
const AUDIENCE: &str = "golem-agent-invocation";
const SIGNATURE_SIZE: usize = 32;
const DOMAIN_PREFIX: &[u8] = b"golem.agent-invocation.token.v1\0";

const FIELD_ISSUER: u8 = 1;
const FIELD_AUDIENCE: u8 = 2;
const FIELD_ACCOUNT: u8 = 3;
const FIELD_PRINCIPAL: u8 = 4;
const FIELD_ISSUED_AT: u8 = 5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum InvocationSessionTokenKind {
    Session = 1,
    Stream = 2,
    Cursor = 3,
}

impl InvocationSessionTokenKind {
    fn from_byte(value: u8) -> Result<Self, InvocationSessionTokenError> {
        match value {
            1 => Ok(Self::Session),
            2 => Ok(Self::Stream),
            3 => Ok(Self::Cursor),
            _ => Err(token_invalid("unknown token kind")),
        }
    }

    fn domain(self) -> &'static [u8] {
        match self {
            Self::Session => b"session\0",
            Self::Stream => b"stream\0",
            Self::Cursor => b"cursor\0",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InvocationSessionTokenBindings {
    pub account: String,
    pub effective_principal: String,
    pub issued_at: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionTokenPayload {
    pub application: String,
    pub environment: String,
    pub agent: String,
    pub idempotency_key: String,
    pub logical_invocation_id: Uuid,
    pub attachment_id: Uuid,
    pub expected_attachment_generation: u64,
    pub callee_incarnation: Uuid,
    pub stream_key_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionAgentIdentity {
    pub component_id: Uuid,
    pub component_revision: u64,
    pub agent_type: String,
    pub agent_id: String,
    pub method: String,
}

pub fn encode_session_agent_identity(
    identity: &SessionAgentIdentity,
) -> Result<String, InvocationSessionTokenError> {
    let mut bytes = Vec::new();
    bytes.push(1);
    bytes.extend_from_slice(identity.component_id.as_bytes());
    bytes.extend_from_slice(&identity.component_revision.to_be_bytes());
    for value in [&identity.agent_type, &identity.agent_id, &identity.method] {
        validate_text(value)?;
        let length: u32 = value
            .len()
            .try_into()
            .map_err(|_| resource_exhausted("agent identity field is too long"))?;
        bytes.extend_from_slice(&length.to_be_bytes());
        bytes.extend_from_slice(value.as_bytes());
    }
    Ok(format!("v1.{}", URL_SAFE_NO_PAD.encode(bytes)))
}

pub fn decode_session_agent_identity(
    encoded: &str,
) -> Result<SessionAgentIdentity, InvocationSessionTokenError> {
    let encoded = encoded
        .strip_prefix("v1.")
        .ok_or_else(|| token_invalid("invalid session agent identity"))?;
    let bytes = URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|_| token_invalid("invalid session agent identity"))?;
    if bytes.len() < 25 || bytes[0] != 1 {
        return Err(token_invalid("invalid session agent identity"));
    }
    let component_id = Uuid::from_slice(&bytes[1..17])
        .map_err(|_| token_invalid("invalid session agent identity"))?;
    let component_revision = u64::from_be_bytes(bytes[17..25].try_into().unwrap());
    let mut offset = 25;
    let agent_type = read_identity_text(&bytes, &mut offset)?;
    let agent_id = read_identity_text(&bytes, &mut offset)?;
    let method = read_identity_text(&bytes, &mut offset)?;
    if offset != bytes.len() {
        return Err(token_invalid("invalid session agent identity"));
    }
    Ok(SessionAgentIdentity {
        component_id,
        component_revision,
        agent_type,
        agent_id,
        method,
    })
}

fn read_identity_text(
    bytes: &[u8],
    offset: &mut usize,
) -> Result<String, InvocationSessionTokenError> {
    let length_end = offset
        .checked_add(4)
        .filter(|end| *end <= bytes.len())
        .ok_or_else(|| token_invalid("invalid session agent identity"))?;
    let length = u32::from_be_bytes(bytes[*offset..length_end].try_into().unwrap()) as usize;
    *offset = length_end;
    let value_end = offset
        .checked_add(length)
        .filter(|end| *end <= bytes.len())
        .ok_or_else(|| token_invalid("invalid session agent identity"))?;
    let value = std::str::from_utf8(&bytes[*offset..value_end])
        .map_err(|_| token_invalid("invalid session agent identity"))?;
    validate_text(value)?;
    *offset = value_end;
    Ok(value.to_string())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum StreamTokenRole {
    Input = 1,
    Output = 2,
}

impl StreamTokenRole {
    fn from_byte(value: u8) -> Result<Self, InvocationSessionTokenError> {
        match value {
            1 => Ok(Self::Input),
            2 => Ok(Self::Output),
            _ => Err(token_invalid("invalid stream role")),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StreamTokenPayload {
    pub parent_logical_invocation_id: Uuid,
    pub durable_stream_id: Uuid,
    pub producer: String,
    pub producer_incarnation: Uuid,
    pub component_revision: u64,
    pub schema_fingerprint: [u8; 32],
    pub role: StreamTokenRole,
    pub durable_mapping_id: Uuid,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CursorTokenPayload {
    pub parent_logical_invocation_id: Uuid,
    pub output_durable_stream_id: Uuid,
    pub durable_offset: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InvocationSessionTokenPayload {
    Session(SessionTokenPayload),
    Stream(StreamTokenPayload),
    Cursor(CursorTokenPayload),
}

impl InvocationSessionTokenPayload {
    pub fn kind(&self) -> InvocationSessionTokenKind {
        match self {
            Self::Session(_) => InvocationSessionTokenKind::Session,
            Self::Stream(_) => InvocationSessionTokenKind::Stream,
            Self::Cursor(_) => InvocationSessionTokenKind::Cursor,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedInvocationSessionToken {
    pub bindings: InvocationSessionTokenBindings,
    pub key_id: String,
    pub payload: InvocationSessionTokenPayload,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InvocationSessionTokenError {
    pub code: PublicErrorCode,
    message: &'static str,
}

impl Display for InvocationSessionTokenError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message)
    }
}

impl std::error::Error for InvocationSessionTokenError {}

#[derive(Clone)]
pub struct InvocationSessionTokenKeyring {
    issuer: String,
    active_key_id: String,
    keys: HashMap<String, Vec<u8>>,
}

impl InvocationSessionTokenKeyring {
    pub fn new(config: &InvocationSessionTokenConfig) -> Result<Self, String> {
        if config.issuer.is_empty() {
            return Err("invocation session token issuer must not be empty".to_string());
        }
        let mut keys = HashMap::new();
        for key in std::iter::once(&config.active_key).chain(&config.verify_only_keys) {
            if key.id.is_empty() || key.id.len() > 64 {
                return Err("invocation session token key id must be 1..64 bytes".to_string());
            }
            if key.key.0.len() < 32 {
                return Err("invocation session HMAC key must be at least 32 bytes".to_string());
            }
            if keys.insert(key.id.clone(), key.key.0.clone()).is_some() {
                return Err("invocation session token key ids must be unique".to_string());
            }
        }
        Ok(Self {
            issuer: config.issuer.clone(),
            active_key_id: config.active_key.id.clone(),
            keys,
        })
    }

    pub fn sign(
        &self,
        bindings: &InvocationSessionTokenBindings,
        payload: &InvocationSessionTokenPayload,
    ) -> Result<String, InvocationSessionTokenError> {
        self.sign_with_key_id(&self.active_key_id, bindings, payload)
    }

    pub fn active_key_id(&self) -> &str {
        &self.active_key_id
    }

    pub fn sign_with_key_id(
        &self,
        key_id: &str,
        bindings: &InvocationSessionTokenBindings,
        payload: &InvocationSessionTokenPayload,
    ) -> Result<String, InvocationSessionTokenError> {
        validate_text(&bindings.account)?;
        validate_text(&bindings.effective_principal)?;
        let key = self
            .keys
            .get(key_id)
            .ok_or_else(|| token_invalid("token signing key is unavailable"))?;
        let mut encoded = Vec::new();
        encoded.push(FORMAT_VERSION);
        encoded.push(payload.kind() as u8);
        encoded.push(key_id.len() as u8);
        encoded.extend_from_slice(key_id.as_bytes());
        write_common_fields(&mut encoded, &self.issuer, bindings)?;
        write_payload_fields(&mut encoded, payload)?;
        let signature = signature(key, payload.kind(), &encoded)?;
        encoded.extend_from_slice(&signature);
        let token = format!("{TOKEN_PREFIX}{}", URL_SAFE_NO_PAD.encode(encoded));
        if token.len() > MAX_TOKEN_SIZE {
            return Err(resource_exhausted("encoded token exceeds 8192 bytes"));
        }
        Ok(token)
    }

    pub fn verify(
        &self,
        token: &str,
        expected_kind: InvocationSessionTokenKind,
        expected_account: &str,
        expected_principal: &str,
    ) -> Result<VerifiedInvocationSessionToken, InvocationSessionTokenError> {
        if token.len() > MAX_TOKEN_SIZE {
            return Err(resource_exhausted("token exceeds 8192 bytes"));
        }
        let body = token
            .strip_prefix(TOKEN_PREFIX)
            .ok_or_else(|| token_invalid("invalid token prefix"))?;
        let decoded = URL_SAFE_NO_PAD
            .decode(body)
            .map_err(|_| token_invalid("invalid token encoding"))?;
        if decoded.len() <= SIGNATURE_SIZE {
            return Err(token_invalid("token is shorter than its signature"));
        }
        let (payload_bytes, signature_bytes) = decoded.split_at(decoded.len() - SIGNATURE_SIZE);
        let header = parse_header(payload_bytes)?;
        if header.kind != expected_kind {
            return Err(token_invalid("token has the wrong kind"));
        }
        let key = self
            .keys
            .get(header.key_id)
            .ok_or_else(|| token_invalid("token key is unavailable"))?;
        verify_signature(key, header.kind, payload_bytes, signature_bytes)?;
        let fields = parse_fields(&payload_bytes[header.fields_offset..])?;
        let bindings = parse_common_fields(&fields)?;
        if read_text(&fields, FIELD_ISSUER)? != self.issuer
            || read_text(&fields, FIELD_AUDIENCE)? != AUDIENCE
        {
            return Err(token_invalid("token issuer or audience does not match"));
        }
        if bindings.account != expected_account
            || bindings.effective_principal != expected_principal
        {
            return Err(InvocationSessionTokenError {
                code: PublicErrorCode::Unauthorized,
                message: "token principal binding does not match",
            });
        }
        let payload = parse_payload_fields(header.kind, header.key_id, &fields)?;
        Ok(VerifiedInvocationSessionToken {
            bindings,
            key_id: header.key_id.to_string(),
            payload,
        })
    }
}

struct TokenHeader<'a> {
    kind: InvocationSessionTokenKind,
    key_id: &'a str,
    fields_offset: usize,
}

fn parse_header(payload: &[u8]) -> Result<TokenHeader<'_>, InvocationSessionTokenError> {
    if payload.len() < 4 || payload[0] != FORMAT_VERSION {
        return Err(token_invalid("unsupported token format"));
    }
    let kind = InvocationSessionTokenKind::from_byte(payload[1])?;
    let key_id_len = payload[2] as usize;
    if !(1..=64).contains(&key_id_len) || payload.len() < 3 + key_id_len + 5 {
        return Err(token_invalid("invalid token key id"));
    }
    let key_id = std::str::from_utf8(&payload[3..3 + key_id_len])
        .map_err(|_| token_invalid("invalid token key id"))?;
    Ok(TokenHeader {
        kind,
        key_id,
        fields_offset: 3 + key_id_len,
    })
}

fn parse_fields(mut bytes: &[u8]) -> Result<BTreeMap<u8, Vec<u8>>, InvocationSessionTokenError> {
    let mut fields = BTreeMap::new();
    let mut previous = 0;
    while !bytes.is_empty() {
        if bytes.len() < 5 {
            return Err(token_invalid("truncated token field"));
        }
        let field_id = bytes[0];
        let length = u32::from_be_bytes(bytes[1..5].try_into().unwrap()) as usize;
        bytes = &bytes[5..];
        if field_id <= previous || length == 0 || bytes.len() < length {
            return Err(token_invalid("invalid token field ordering or length"));
        }
        fields.insert(field_id, bytes[..length].to_vec());
        bytes = &bytes[length..];
        previous = field_id;
    }
    Ok(fields)
}

fn write_common_fields(
    output: &mut Vec<u8>,
    issuer: &str,
    bindings: &InvocationSessionTokenBindings,
) -> Result<(), InvocationSessionTokenError> {
    write_text(output, FIELD_ISSUER, issuer)?;
    write_text(output, FIELD_AUDIENCE, AUDIENCE)?;
    write_text(output, FIELD_ACCOUNT, &bindings.account)?;
    write_text(output, FIELD_PRINCIPAL, &bindings.effective_principal)?;
    write_u64(output, FIELD_ISSUED_AT, bindings.issued_at);
    Ok(())
}

fn write_payload_fields(
    output: &mut Vec<u8>,
    payload: &InvocationSessionTokenPayload,
) -> Result<(), InvocationSessionTokenError> {
    match payload {
        InvocationSessionTokenPayload::Session(payload) => {
            write_text(output, 16, &payload.application)?;
            write_text(output, 17, &payload.environment)?;
            write_text(output, 18, &payload.agent)?;
            write_text(output, 19, &payload.idempotency_key)?;
            write_uuid(output, 20, payload.logical_invocation_id);
            write_uuid(output, 21, payload.attachment_id);
            write_u64(output, 22, payload.expected_attachment_generation);
            write_uuid(output, 23, payload.callee_incarnation);
            write_text(output, 24, &payload.stream_key_id)?;
        }
        InvocationSessionTokenPayload::Stream(payload) => {
            write_uuid(output, 32, payload.parent_logical_invocation_id);
            write_uuid(output, 33, payload.durable_stream_id);
            write_text(output, 34, &payload.producer)?;
            write_uuid(output, 35, payload.producer_incarnation);
            write_u64(output, 36, payload.component_revision);
            write_field(output, 37, &payload.schema_fingerprint)?;
            write_field(output, 38, &[payload.role as u8])?;
            write_uuid(output, 39, payload.durable_mapping_id);
        }
        InvocationSessionTokenPayload::Cursor(payload) => {
            write_uuid(output, 48, payload.parent_logical_invocation_id);
            write_uuid(output, 49, payload.output_durable_stream_id);
            write_field(output, 50, &payload.durable_offset)?;
        }
    }
    Ok(())
}

fn parse_common_fields(
    fields: &BTreeMap<u8, Vec<u8>>,
) -> Result<InvocationSessionTokenBindings, InvocationSessionTokenError> {
    Ok(InvocationSessionTokenBindings {
        account: read_text(fields, FIELD_ACCOUNT)?.to_string(),
        effective_principal: read_text(fields, FIELD_PRINCIPAL)?.to_string(),
        issued_at: read_u64(fields, FIELD_ISSUED_AT)?,
    })
}

fn parse_payload_fields(
    kind: InvocationSessionTokenKind,
    key_id: &str,
    fields: &BTreeMap<u8, Vec<u8>>,
) -> Result<InvocationSessionTokenPayload, InvocationSessionTokenError> {
    let expected = match kind {
        InvocationSessionTokenKind::Session => {
            &[1, 2, 3, 4, 5, 16, 17, 18, 19, 20, 21, 22, 23, 24][..]
        }
        InvocationSessionTokenKind::Stream => &[1, 2, 3, 4, 5, 32, 33, 34, 35, 36, 37, 38, 39][..],
        InvocationSessionTokenKind::Cursor => &[1, 2, 3, 4, 5, 48, 49, 50][..],
    };
    let legacy_session = &[1, 2, 3, 4, 5, 16, 17, 18, 19, 20, 21, 22, 23][..];
    if fields.keys().copied().ne(expected.iter().copied())
        && !(kind == InvocationSessionTokenKind::Session
            && fields.keys().copied().eq(legacy_session.iter().copied()))
    {
        return Err(token_invalid("token fields do not match its kind"));
    }
    match kind {
        InvocationSessionTokenKind::Session => Ok(InvocationSessionTokenPayload::Session(
            SessionTokenPayload {
                application: read_text(fields, 16)?.to_string(),
                environment: read_text(fields, 17)?.to_string(),
                agent: read_text(fields, 18)?.to_string(),
                idempotency_key: read_text(fields, 19)?.to_string(),
                logical_invocation_id: read_uuid(fields, 20)?,
                attachment_id: read_uuid(fields, 21)?,
                expected_attachment_generation: read_u64(fields, 22)?,
                callee_incarnation: read_uuid(fields, 23)?,
                stream_key_id: fields
                    .get(&24)
                    .map(|_| read_text(fields, 24).map(ToString::to_string))
                    .transpose()?
                    .unwrap_or_else(|| key_id.to_string()),
            },
        )),
        InvocationSessionTokenKind::Stream => {
            let fingerprint: [u8; 32] = read_field(fields, 37)?
                .try_into()
                .map_err(|_| token_invalid("invalid schema fingerprint"))?;
            let role = read_field(fields, 38)?;
            if role.len() != 1 {
                return Err(token_invalid("invalid stream role"));
            }
            Ok(InvocationSessionTokenPayload::Stream(StreamTokenPayload {
                parent_logical_invocation_id: read_uuid(fields, 32)?,
                durable_stream_id: read_uuid(fields, 33)?,
                producer: read_text(fields, 34)?.to_string(),
                producer_incarnation: read_uuid(fields, 35)?,
                component_revision: read_u64(fields, 36)?,
                schema_fingerprint: fingerprint,
                role: StreamTokenRole::from_byte(role[0])?,
                durable_mapping_id: read_uuid(fields, 39)?,
            }))
        }
        InvocationSessionTokenKind::Cursor => {
            Ok(InvocationSessionTokenPayload::Cursor(CursorTokenPayload {
                parent_logical_invocation_id: read_uuid(fields, 48)?,
                output_durable_stream_id: read_uuid(fields, 49)?,
                durable_offset: read_field(fields, 50)?.to_vec(),
            }))
        }
    }
}

fn write_text(
    output: &mut Vec<u8>,
    field_id: u8,
    value: &str,
) -> Result<(), InvocationSessionTokenError> {
    validate_text(value)?;
    write_field(output, field_id, value.as_bytes())
}

fn validate_text(value: &str) -> Result<(), InvocationSessionTokenError> {
    if value.is_empty() {
        Err(token_invalid("token text field must not be empty"))
    } else {
        Ok(())
    }
}

fn write_uuid(output: &mut Vec<u8>, field_id: u8, value: Uuid) {
    write_field(output, field_id, value.as_bytes()).expect("UUID has a bounded non-empty length");
}

fn write_u64(output: &mut Vec<u8>, field_id: u8, value: u64) {
    write_field(output, field_id, &value.to_be_bytes())
        .expect("u64 has a bounded non-empty length");
}

fn write_field(
    output: &mut Vec<u8>,
    field_id: u8,
    value: &[u8],
) -> Result<(), InvocationSessionTokenError> {
    if value.is_empty() || value.len() > u32::MAX as usize {
        return Err(token_invalid("invalid token field length"));
    }
    output.push(field_id);
    output.extend_from_slice(&(value.len() as u32).to_be_bytes());
    output.extend_from_slice(value);
    Ok(())
}

fn read_field(
    fields: &BTreeMap<u8, Vec<u8>>,
    field_id: u8,
) -> Result<&[u8], InvocationSessionTokenError> {
    fields
        .get(&field_id)
        .map(Vec::as_slice)
        .ok_or_else(|| token_invalid("missing token field"))
}

fn read_text(
    fields: &BTreeMap<u8, Vec<u8>>,
    field_id: u8,
) -> Result<&str, InvocationSessionTokenError> {
    std::str::from_utf8(read_field(fields, field_id)?)
        .map_err(|_| token_invalid("token text field is not UTF-8"))
}

fn read_uuid(
    fields: &BTreeMap<u8, Vec<u8>>,
    field_id: u8,
) -> Result<Uuid, InvocationSessionTokenError> {
    Uuid::from_slice(read_field(fields, field_id)?)
        .map_err(|_| token_invalid("invalid token UUID field"))
}

fn read_u64(
    fields: &BTreeMap<u8, Vec<u8>>,
    field_id: u8,
) -> Result<u64, InvocationSessionTokenError> {
    let bytes: [u8; 8] = read_field(fields, field_id)?
        .try_into()
        .map_err(|_| token_invalid("invalid token u64 field"))?;
    Ok(u64::from_be_bytes(bytes))
}

fn signature(
    key: &[u8],
    kind: InvocationSessionTokenKind,
    payload: &[u8],
) -> Result<[u8; SIGNATURE_SIZE], InvocationSessionTokenError> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key)
        .map_err(|_| token_invalid("invalid token signing key"))?;
    mac.update(DOMAIN_PREFIX);
    mac.update(kind.domain());
    mac.update(payload);
    Ok(mac.finalize().into_bytes().into())
}

fn verify_signature(
    key: &[u8],
    kind: InvocationSessionTokenKind,
    payload: &[u8],
    candidate: &[u8],
) -> Result<(), InvocationSessionTokenError> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key)
        .map_err(|_| token_invalid("invalid token signing key"))?;
    mac.update(DOMAIN_PREFIX);
    mac.update(kind.domain());
    mac.update(payload);
    mac.verify_slice(candidate)
        .map_err(|_| token_invalid("invalid token signature"))
}

fn token_invalid(message: &'static str) -> InvocationSessionTokenError {
    InvocationSessionTokenError {
        code: PublicErrorCode::TokenInvalid,
        message,
    }
}

fn resource_exhausted(message: &'static str) -> InvocationSessionTokenError {
    InvocationSessionTokenError {
        code: PublicErrorCode::ResourceExhausted,
        message,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CursorTokenPayload, InvocationSessionTokenBindings, InvocationSessionTokenKeyring,
        InvocationSessionTokenKind, InvocationSessionTokenPayload, SessionTokenPayload,
        StreamTokenPayload, StreamTokenRole,
    };
    use crate::config::{InvocationSessionTokenConfig, InvocationSessionTokenKeyConfig};
    use base64::Engine;
    use golem_common::base_model::auth::TokenSecret;
    use golem_common::model::base64::Base64;
    use golem_common::model::invocation_session_public::PublicErrorCode;
    use serde::Deserialize;
    use std::io::Write;
    use std::sync::{Arc, Mutex};
    use test_r::test;
    use tracing_subscriber::fmt::MakeWriter;
    use uuid::Uuid;

    #[derive(Clone, Default)]
    struct CapturedLogs(Arc<Mutex<Vec<u8>>>);

    impl CapturedLogs {
        fn text(&self) -> String {
            String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
        }
    }

    struct CapturedLogWriter(Arc<Mutex<Vec<u8>>>);

    impl Write for CapturedLogWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> MakeWriter<'a> for CapturedLogs {
        type Writer = CapturedLogWriter;

        fn make_writer(&'a self) -> Self::Writer {
            CapturedLogWriter(self.0.clone())
        }
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct TokenFixture {
        hmac_sha256_key_hex: String,
        key_id: String,
        vectors: Vec<TokenVector>,
        rotation: RotationFixture,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct TokenVector {
        name: String,
        payload_hex: String,
        signature_hex: String,
        token: String,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct RotationFixture {
        verify_only_keys: Vec<KeyFixture>,
        verify_only_token: ExpectedToken,
        unknown_key_token: ExpectedToken,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct KeyFixture {
        id: String,
        key_hex: String,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct ExpectedToken {
        token: String,
    }

    fn fixture() -> TokenFixture {
        serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../golem-client/tests/fixtures/stream-session-v1/tokens.json"
        )))
        .unwrap()
    }

    fn keyring(fixture: &TokenFixture) -> InvocationSessionTokenKeyring {
        InvocationSessionTokenKeyring::new(&InvocationSessionTokenConfig {
            issuer: "test-deployment".to_string(),
            active_key: InvocationSessionTokenKeyConfig {
                id: fixture.key_id.clone(),
                key: Base64(decode_hex(&fixture.hmac_sha256_key_hex)),
            },
            verify_only_keys: fixture
                .rotation
                .verify_only_keys
                .iter()
                .map(|key| InvocationSessionTokenKeyConfig {
                    id: key.id.clone(),
                    key: Base64(decode_hex(&key.key_hex)),
                })
                .collect(),
        })
        .unwrap()
    }

    fn bindings() -> InvocationSessionTokenBindings {
        InvocationSessionTokenBindings {
            account: "account-123".to_string(),
            effective_principal: "user:account-123".to_string(),
            issued_at: 0x6a90_cf80,
        }
    }

    fn payload(name: &str) -> InvocationSessionTokenPayload {
        let logical = uuid("11111111-1111-4111-8111-111111111111");
        let incarnation = uuid("33333333-3333-4333-8333-333333333333");
        match name {
            "session" => InvocationSessionTokenPayload::Session(SessionTokenPayload {
                application: "demo".to_string(),
                environment: "default".to_string(),
                agent: "media/encoder".to_string(),
                idempotency_key: "upload-42".to_string(),
                logical_invocation_id: logical,
                attachment_id: uuid("22222222-2222-4222-8222-222222222222"),
                expected_attachment_generation: 7,
                callee_incarnation: incarnation,
                stream_key_id: "test-key".to_string(),
            }),
            "output-stream" => InvocationSessionTokenPayload::Stream(StreamTokenPayload {
                parent_logical_invocation_id: logical,
                durable_stream_id: uuid("44444444-4444-4444-8444-444444444444"),
                producer: "media/encoder".to_string(),
                producer_incarnation: incarnation,
                component_revision: 42,
                schema_fingerprint: sequential::<32>(0xa0),
                role: StreamTokenRole::Output,
                durable_mapping_id: uuid("55555555-5555-4555-8555-555555555555"),
            }),
            "input-stream" => InvocationSessionTokenPayload::Stream(StreamTokenPayload {
                parent_logical_invocation_id: logical,
                durable_stream_id: uuid("66666666-6666-4666-8666-666666666666"),
                producer: "external-client".to_string(),
                producer_incarnation: incarnation,
                component_revision: 42,
                schema_fingerprint: sequential::<32>(0xc0),
                role: StreamTokenRole::Input,
                durable_mapping_id: uuid("77777777-7777-4777-8777-777777777777"),
            }),
            "output-cursor" => InvocationSessionTokenPayload::Cursor(CursorTokenPayload {
                parent_logical_invocation_id: logical,
                output_durable_stream_id: uuid("44444444-4444-4444-8444-444444444444"),
                durable_offset: sequential::<24>(0).to_vec(),
            }),
            _ => panic!("unknown token fixture"),
        }
    }

    #[test]
    fn golden_tokens_match_tlv_and_signature_exactly() {
        let fixture = fixture();
        let keyring = keyring(&fixture);
        for vector in &fixture.vectors {
            let token = keyring.sign(&bindings(), &payload(&vector.name)).unwrap();
            assert_eq!(token, vector.token, "{}", vector.name);

            let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(token.strip_prefix("gai1_").unwrap())
                .unwrap();
            let payload_end = decoded.len() - 32;
            assert_eq!(decoded[..payload_end], decode_hex(&vector.payload_hex));
            assert_eq!(decoded[payload_end..], decode_hex(&vector.signature_hex));

            let expected_kind = payload(&vector.name).kind();
            let verified = keyring
                .verify(&token, expected_kind, "account-123", "user:account-123")
                .unwrap();
            assert_eq!(verified.bindings, bindings());
            assert_eq!(verified.key_id, fixture.key_id);
            assert_eq!(verified.payload, payload(&vector.name));
        }
    }

    #[test]
    fn verify_only_key_is_accepted_but_unknown_key_is_rejected() {
        let fixture = fixture();
        let keyring = keyring(&fixture);
        keyring
            .verify(
                &fixture.rotation.verify_only_token.token,
                InvocationSessionTokenKind::Session,
                "account-123",
                "user:account-123",
            )
            .unwrap();
        assert_eq!(
            keyring
                .verify(
                    &fixture.rotation.unknown_key_token.token,
                    InvocationSessionTokenKind::Session,
                    "account-123",
                    "user:account-123",
                )
                .unwrap_err()
                .code,
            PublicErrorCode::TokenInvalid
        );
    }

    #[test]
    fn token_limits_and_principal_binding_are_enforced() {
        let fixture = fixture();
        let keyring = keyring(&fixture);
        let token = &fixture.vectors[0].token;
        assert_eq!(
            keyring
                .verify(
                    token,
                    InvocationSessionTokenKind::Session,
                    "another-account",
                    "user:account-123",
                )
                .unwrap_err()
                .code,
            PublicErrorCode::Unauthorized
        );
        assert_eq!(
            keyring
                .verify(
                    &format!("gai1_{}", "a".repeat(8193)),
                    InvocationSessionTokenKind::Session,
                    "account-123",
                    "user:account-123",
                )
                .unwrap_err()
                .code,
            PublicErrorCode::ResourceExhausted
        );
    }

    #[test]
    fn verified_signing_context_preserves_stream_tokens_across_key_rotation() {
        let old_key = InvocationSessionTokenKeyConfig {
            id: "old".to_string(),
            key: Base64(vec![7; 32]),
        };
        let old_keyring = InvocationSessionTokenKeyring::new(&InvocationSessionTokenConfig {
            issuer: "test-deployment".to_string(),
            active_key: old_key.clone(),
            verify_only_keys: Vec::new(),
        })
        .unwrap();
        let stream_payload = payload("output-stream");
        let original = old_keyring.sign(&bindings(), &stream_payload).unwrap();

        let rotated = InvocationSessionTokenKeyring::new(&InvocationSessionTokenConfig {
            issuer: "test-deployment".to_string(),
            active_key: InvocationSessionTokenKeyConfig {
                id: "new".to_string(),
                key: Base64(vec![9; 32]),
            },
            verify_only_keys: vec![old_key],
        })
        .unwrap();
        let verified = rotated
            .verify(
                &original,
                InvocationSessionTokenKind::Stream,
                "account-123",
                "user:account-123",
            )
            .unwrap();
        let reconstructed = rotated
            .sign_with_key_id(&verified.key_id, &verified.bindings, &verified.payload)
            .unwrap();

        assert_eq!(reconstructed, original);
    }

    #[test]
    fn credentials_and_capability_tokens_are_absent_from_trace_logs() {
        let fixture = fixture();
        let keyring = keyring(&fixture);
        let bearer = TokenSecret::trusted("bearer-credential-sentinel-1234".to_string());
        let logs = CapturedLogs::default();
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_max_level(tracing::Level::TRACE)
            .with_writer(logs.clone())
            .finish();

        let (session_token, stream_token, cursor_token) =
            tracing::subscriber::with_default(subscriber, || {
                tracing::trace!(bearer = ?bearer, "invocation session log capture probe");
                let session_token = keyring.sign(&bindings(), &payload("session")).unwrap();
                let stream_token = keyring
                    .sign(&bindings(), &payload("output-stream"))
                    .unwrap();
                let cursor_token = keyring
                    .sign(&bindings(), &payload("output-cursor"))
                    .unwrap();
                for (token, kind) in [
                    (&session_token, InvocationSessionTokenKind::Session),
                    (&stream_token, InvocationSessionTokenKind::Stream),
                    (&cursor_token, InvocationSessionTokenKind::Cursor),
                ] {
                    keyring
                        .verify(token, kind, "account-123", "user:account-123")
                        .unwrap();
                }
                (session_token, stream_token, cursor_token)
            });

        let rendered = logs.text();
        assert!(rendered.contains("invocation session log capture probe"));
        assert!(rendered.contains("*******"));
        for secret in [
            bearer.secret(),
            session_token.as_str(),
            stream_token.as_str(),
            cursor_token.as_str(),
        ] {
            assert!(!rendered.contains(secret), "secret leaked into trace logs");
        }
    }

    fn uuid(value: &str) -> Uuid {
        value.parse().unwrap()
    }

    fn sequential<const N: usize>(start: u8) -> [u8; N] {
        std::array::from_fn(|index| start.wrapping_add(index as u8))
    }

    fn decode_hex(value: &str) -> Vec<u8> {
        assert_eq!(value.len() % 2, 0);
        value
            .as_bytes()
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| {
                let pair = std::str::from_utf8(pair).unwrap();
                u8::from_str_radix(pair, 16).unwrap()
            })
            .collect()
    }
}
