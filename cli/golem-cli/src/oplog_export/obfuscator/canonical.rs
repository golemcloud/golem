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

//! Canonical bytes of a normalised value: the token input.
//!
//! The encoding depends only on the normalised value, never on the source type or on JSON.
//! Field names, type names, `Text.language` and `Binary.mime_type` are not part of it, so
//! `String`, `Text`, `Url`, `Path`, `Char` and `Uuid` texts share one encoding, every integer
//! kind encodes as `i128`, and a typed value and its schema-less form encode the same way. Map
//! entries are sorted by their encoded bytes; every other collection keeps its order.

use golem_common::schema::{ResultValuePayload, SchemaValue};

const TAG_BOOL: u8 = 0x01;
const TAG_INT: u8 = 0x02;
const TAG_FLOAT: u8 = 0x03;
const TAG_STR: u8 = 0x04;
const TAG_RECORD: u8 = 0x05;
const TAG_VARIANT: u8 = 0x06;
const TAG_ENUM: u8 = 0x07;
const TAG_FLAGS: u8 = 0x08;
const TAG_TUPLE: u8 = 0x09;
const TAG_LIST: u8 = 0x0A;
const TAG_FIXED_LIST: u8 = 0x0B;
const TAG_MAP: u8 = 0x0C;
const TAG_OPTION: u8 = 0x0D;
const TAG_RESULT: u8 = 0x0E;
const TAG_BINARY: u8 = 0x0F;
const TAG_DATETIME: u8 = 0x10;
const TAG_DURATION: u8 = 0x11;
const TAG_QUANTITY: u8 = 0x12;
const TAG_UNION: u8 = 0x13;
const TAG_HOST_MANAGED: u8 = 0x14;
const TAG_STREAM: u8 = 0x15;

pub(super) fn encode(value: &SchemaValue) -> Vec<u8> {
    let mut out = Vec::new();
    write(&mut out, value);
    out
}

/// The canonical bytes of a string (and of every text-like value).
pub(super) fn encode_str(text: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len() + 5);
    write_str(&mut out, text);
    out
}

fn write_len(out: &mut Vec<u8>, len: usize) {
    out.extend_from_slice(&(len as u32).to_be_bytes());
}

fn write_str(out: &mut Vec<u8>, text: &str) {
    out.push(TAG_STR);
    write_len(out, text.len());
    out.extend_from_slice(text.as_bytes());
}

fn write_int(out: &mut Vec<u8>, value: i128) {
    out.push(TAG_INT);
    out.extend_from_slice(&value.to_be_bytes());
}

fn write_float(out: &mut Vec<u8>, value: f64) {
    out.push(TAG_FLOAT);
    let bits = if value.is_nan() {
        f64::NAN.to_bits()
    } else {
        value.to_bits()
    };
    out.extend_from_slice(&bits.to_be_bytes());
}

fn write_seq(out: &mut Vec<u8>, tag: u8, elements: &[SchemaValue]) {
    out.push(tag);
    write_len(out, elements.len());
    for element in elements {
        write(out, element);
    }
}

fn write_optional(out: &mut Vec<u8>, value: Option<&SchemaValue>) {
    match value {
        Some(value) => {
            out.push(1);
            write(out, value);
        }
        None => out.push(0),
    }
}

fn write(out: &mut Vec<u8>, value: &SchemaValue) {
    stacker::maybe_grow(64 * 1024, 1024 * 1024, || match value {
        SchemaValue::Bool(v) => {
            out.push(TAG_BOOL);
            out.push(u8::from(*v));
        }
        SchemaValue::S8(v) => write_int(out, *v as i128),
        SchemaValue::S16(v) => write_int(out, *v as i128),
        SchemaValue::S32(v) => write_int(out, *v as i128),
        SchemaValue::S64(v) => write_int(out, *v as i128),
        SchemaValue::U8(v) => write_int(out, *v as i128),
        SchemaValue::U16(v) => write_int(out, *v as i128),
        SchemaValue::U32(v) => write_int(out, *v as i128),
        SchemaValue::U64(v) => write_int(out, *v as i128),
        SchemaValue::F32(v) => write_float(out, *v as f64),
        SchemaValue::F64(v) => write_float(out, *v),
        SchemaValue::Char(c) => write_str(out, c.encode_utf8(&mut [0u8; 4])),
        SchemaValue::String(s) => write_str(out, s),
        SchemaValue::Text(text) => write_str(out, &text.text),
        SchemaValue::Path { path } => write_str(out, path),
        SchemaValue::Url { url } => write_str(out, url),
        SchemaValue::Uuid(uuid) => write_str(out, &uuid.hyphenated().to_string()),
        SchemaValue::Record { fields } => write_seq(out, TAG_RECORD, fields),
        SchemaValue::Tuple { elements } => write_seq(out, TAG_TUPLE, elements),
        SchemaValue::List { elements } => write_seq(out, TAG_LIST, elements),
        SchemaValue::FixedList { elements } => write_seq(out, TAG_FIXED_LIST, elements),
        SchemaValue::Variant(payload) => {
            out.push(TAG_VARIANT);
            out.extend_from_slice(&payload.case.to_be_bytes());
            write_optional(out, payload.payload.as_deref());
        }
        SchemaValue::Enum { case } => {
            out.push(TAG_ENUM);
            out.extend_from_slice(&case.to_be_bytes());
        }
        SchemaValue::Flags { bits } => {
            out.push(TAG_FLAGS);
            write_len(out, bits.len());
            out.extend(bits.iter().map(|bit| u8::from(*bit)));
        }
        SchemaValue::Map { entries } => {
            let mut encoded: Vec<(Vec<u8>, Vec<u8>)> = entries
                .iter()
                .map(|(key, value)| (encode(key), encode(value)))
                .collect();
            encoded.sort();
            out.push(TAG_MAP);
            write_len(out, encoded.len());
            for (key, value) in encoded {
                out.extend_from_slice(&key);
                out.extend_from_slice(&value);
            }
        }
        SchemaValue::Option { inner } => {
            out.push(TAG_OPTION);
            write_optional(out, inner.as_deref());
        }
        SchemaValue::Result(result) => {
            out.push(TAG_RESULT);
            match result {
                ResultValuePayload::Ok { value } => {
                    out.push(0);
                    write_optional(out, value.as_deref());
                }
                ResultValuePayload::Err { value } => {
                    out.push(1);
                    write_optional(out, value.as_deref());
                }
            }
        }
        SchemaValue::Binary(binary) => {
            out.push(TAG_BINARY);
            write_len(out, binary.bytes.len());
            out.extend_from_slice(&binary.bytes);
        }
        SchemaValue::Datetime { value } => {
            out.push(TAG_DATETIME);
            out.extend_from_slice(&value.timestamp().to_be_bytes());
            out.extend_from_slice(&value.timestamp_subsec_nanos().to_be_bytes());
        }
        SchemaValue::Duration(duration) => {
            out.push(TAG_DURATION);
            out.extend_from_slice(&duration.nanoseconds.to_be_bytes());
        }
        SchemaValue::Quantity(quantity) => {
            out.push(TAG_QUANTITY);
            out.extend_from_slice(&quantity.mantissa.to_be_bytes());
            out.extend_from_slice(&quantity.scale.to_be_bytes());
            write_str(out, &quantity.unit);
        }
        SchemaValue::Union(union) => {
            out.push(TAG_UNION);
            write_str(out, &union.tag);
            write(out, &union.body);
        }
        SchemaValue::Secret(_) => {
            out.push(TAG_HOST_MANAGED);
            out.push(0);
        }
        SchemaValue::QuotaToken(_) => {
            out.push(TAG_HOST_MANAGED);
            out.push(1);
        }
        SchemaValue::PermissionCard(_) => {
            out.push(TAG_HOST_MANAGED);
            out.push(2);
        }
        SchemaValue::Stream(_) => out.push(TAG_STREAM),
    })
}
