// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use crate::model::agent::{
    DataValue, ElementValue, ElementValues, NamedElementValue, NamedElementValues,
};
use itertools::Itertools;
use std::io::Write;
use thiserror::Error;
use wasm_wave::lex::Keyword;
use wasm_wave::wasm::{WasmTypeKind, WasmValue};

/// Based on the normal WAVE writer but does not write whitespaces,
/// so it can be used as AgentId
struct CompactWaveWriter<W> {
    inner: W,
}

impl<W: Write> CompactWaveWriter<W> {
    pub fn new(w: W) -> Self {
        Self { inner: w }
    }

    pub fn write_value<V>(&mut self, val: &V) -> Result<(), CompactWaveWriterError>
    where
        V: WasmValue,
    {
        match val.kind() {
            WasmTypeKind::Bool => self.write_str(if val.unwrap_bool() { "true" } else { "false" }),
            WasmTypeKind::S8 => self.write_display(val.unwrap_s8()),
            WasmTypeKind::S16 => self.write_display(val.unwrap_s16()),
            WasmTypeKind::S32 => self.write_display(val.unwrap_s32()),
            WasmTypeKind::S64 => self.write_display(val.unwrap_s64()),
            WasmTypeKind::U8 => self.write_display(val.unwrap_u8()),
            WasmTypeKind::U16 => self.write_display(val.unwrap_u16()),
            WasmTypeKind::U32 => self.write_display(val.unwrap_u32()),
            WasmTypeKind::U64 => self.write_display(val.unwrap_u64()),
            WasmTypeKind::F32 => {
                let f = val.unwrap_f32();
                if f.is_nan() {
                    self.write_str("nan") // Display is "NaN"
                } else {
                    self.write_display(f)
                }
            }
            WasmTypeKind::F64 => {
                let f = val.unwrap_f64();
                if f.is_nan() {
                    self.write_str("nan") // Display is "NaN"
                } else {
                    self.write_display(f)
                }
            }
            WasmTypeKind::Char => {
                self.write_str("'")?;
                self.write_char(val.unwrap_char())?;
                self.write_str("'")
            }
            WasmTypeKind::String => {
                self.write_str("\"")?;
                for ch in val.unwrap_string().chars() {
                    self.write_char(ch)?;
                }
                self.write_str("\"")
            }
            WasmTypeKind::List => {
                self.write_str("[")?;
                for (idx, val) in val.unwrap_list().enumerate() {
                    if idx != 0 {
                        self.write_str(",")?;
                    }
                    self.write_value(&*val)?;
                }
                self.write_str("]")
            }
            WasmTypeKind::FixedSizeList => {
                self.write_str("[")?;
                for (idx, val) in val.unwrap_list().enumerate() {
                    if idx != 0 {
                        self.write_str(",")?;
                    }
                    self.write_value(&*val)?;
                }
                self.write_str("]")
            }
            WasmTypeKind::Record => {
                self.write_str("{")?;
                let mut first = true;
                for (name, val) in val.unwrap_record() {
                    if !matches!(val.kind(), WasmTypeKind::Option) || val.unwrap_option().is_some()
                    {
                        if first {
                            first = false;
                        } else {
                            self.write_str(",")?;
                        }
                        self.write_str(name)?;
                        self.write_str(":")?;
                        self.write_value(&*val)?;
                    }
                }
                if first {
                    self.write_str(":")?;
                }
                self.write_str("}")
            }
            WasmTypeKind::Tuple => {
                self.write_str("(")?;
                for (idx, val) in val.unwrap_tuple().enumerate() {
                    if idx != 0 {
                        self.write_str(",")?;
                    }
                    self.write_value(&*val)?;
                }
                self.write_str(")")
            }
            WasmTypeKind::Variant => {
                let (name, val) = val.unwrap_variant();
                if Keyword::decode(&name).is_some() {
                    self.write_char('%')?;
                }
                self.write_str(name)?;
                if let Some(val) = val {
                    self.write_str("(")?;
                    self.write_value(&*val)?;
                    self.write_str(")")?;
                }
                Ok(())
            }
            WasmTypeKind::Enum => {
                let case = val.unwrap_enum();
                if Keyword::decode(&case).is_some() {
                    self.write_char('%')?;
                }
                self.write_str(case)
            }
            WasmTypeKind::Option => match val.unwrap_option() {
                Some(val) => {
                    self.write_str("some(")?;
                    self.write_value(&*val)?;
                    self.write_str(")")
                }
                None => self.write_str("none"),
            },
            WasmTypeKind::Result => {
                let (name, val) = match val.unwrap_result() {
                    Ok(val) => ("ok", val),
                    Err(val) => ("err", val),
                };
                self.write_str(name)?;
                if let Some(val) = val {
                    self.write_str("(")?;
                    self.write_value(&*val)?;
                    self.write_str(")")?;
                }
                Ok(())
            }
            WasmTypeKind::Flags => {
                self.write_str("{")?;
                for (idx, name) in val.unwrap_flags().enumerate() {
                    if idx != 0 {
                        self.write_str(",")?;
                    }
                    self.write_str(name)?;
                }
                self.write_str("}")?;
                Ok(())
            }
            WasmTypeKind::Unsupported => panic!("unsupported value type"),
            _ => panic!("unknown value kind: {}", val.kind()),
        }
    }

    fn write_str(&mut self, s: impl AsRef<str>) -> Result<(), CompactWaveWriterError> {
        self.inner.write_all(s.as_ref().as_bytes())?;
        Ok(())
    }

    fn write_display(&mut self, d: impl std::fmt::Display) -> Result<(), CompactWaveWriterError> {
        write!(self.inner, "{d}")?;
        Ok(())
    }

    fn write_char(&mut self, ch: char) -> Result<(), CompactWaveWriterError> {
        if "\\\"\'\t\r\n".contains(ch) {
            write!(self.inner, "{}", ch.escape_default())?;
        } else if ch.is_control() {
            write!(self.inner, "{}", ch.escape_unicode())?;
        } else {
            write!(self.inner, "{}", ch.escape_debug())?;
        }
        Ok(())
    }
}

impl<W> AsMut<W> for CompactWaveWriter<W> {
    fn as_mut(&mut self) -> &mut W {
        &mut self.inner
    }
}

fn wave_to_compact_string(val: &impl WasmValue) -> Result<String, CompactWaveWriterError> {
    let mut buf = vec![];
    CompactWaveWriter::new(&mut buf).write_value(val)?;
    Ok(String::from_utf8(buf).unwrap_or_else(|err| panic!("invalid UTF-8: {err:?}")))
}

#[derive(Debug, Error)]
#[non_exhaustive]
enum CompactWaveWriterError {
    #[error("write failed: {0}")]
    Io(#[from] std::io::Error),
}

pub trait ToCompactString {
    fn to_compact_string(&self) -> String;
}

impl ToCompactString for DataValue {
    fn to_compact_string(&self) -> String {
        match self {
            DataValue::Tuple(elems) => elems.to_compact_string(),
            DataValue::Multimodal(elems) => elems.to_compact_string(),
        }
    }
}

impl ToCompactString for ElementValues {
    fn to_compact_string(&self) -> String {
        self.elements
            .iter()
            .map(|elem| elem.to_compact_string())
            .join(",")
    }
}

impl ToCompactString for ElementValue {
    fn to_compact_string(&self) -> String {
        match self {
            ElementValue::ComponentModel(value) => value.to_compact_string(),
            // TODO: also encode as wave for escaping?
            ElementValue::UnstructuredText(text_reference) => text_reference.to_string(),
            // TODO: also encode as wave for escaping?
            ElementValue::UnstructuredBinary(binary_reference) => binary_reference.to_string(),
        }
    }
}

impl ToCompactString for NamedElementValues {
    fn to_compact_string(&self) -> String {
        self.elements
            .iter()
            .map(|elem| elem.to_compact_string())
            .join(",")
    }
}

impl ToCompactString for NamedElementValue {
    fn to_compact_string(&self) -> String {
        // TODO: recheck this, Display uses this
        format!("{}({})", self.name, self.value.to_compact_string())
    }
}

impl<WV: WasmValue> ToCompactString for WV {
    fn to_compact_string(&self) -> String {
        wave_to_compact_string(self).unwrap_or_default()
    }
}
