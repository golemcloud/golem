use anyhow::{Context, anyhow, bail};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::ops::Range;
use wasmparser::BinaryReader;

use crate::gzip::gzip_size;

pub const CORE_HEADER: [u8; 8] = *b"\0asm\x01\0\0\0";
pub const COMPONENT_HEADER: [u8; 8] = *b"\0asm\x0d\0\x01\0";

pub const CUSTOM_SECTION: u8 = 0;
pub const CORE_CODE_SECTION: u8 = 10;
pub const CORE_DATA_SECTION: u8 = 11;
/// Component section ids that embed a core module and a nested component.
pub const COMPONENT_CORE_MODULE_SECTION: u8 = 1;
pub const COMPONENT_COMPONENT_SECTION: u8 = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryKind {
    Core,
    Component,
}

pub fn binary_kind(data: &[u8]) -> anyhow::Result<BinaryKind> {
    match data.get(..8) {
        Some(header) if header == CORE_HEADER => Ok(BinaryKind::Core),
        Some(header) if header == COMPONENT_HEADER => Ok(BinaryKind::Component),
        _ => bail!("expected a core module or component"),
    }
}

/// One top-level section of a core module or component: its id, the range of the whole section
/// including the id and the length prefix, and the range of its payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawSection {
    pub id: u8,
    pub whole: Range<usize>,
    pub payload: Range<usize>,
}

pub fn raw_sections(data: &[u8]) -> anyhow::Result<Vec<RawSection>> {
    binary_kind(data)?;
    let mut sections = Vec::new();
    let mut offset = 8;
    while offset < data.len() {
        let id = data[offset];
        let mut reader = BinaryReader::new(&data[offset + 1..], 0);
        let size = reader
            .read_var_u32()
            .map_err(|error| anyhow!("invalid section length: {error}"))?
            as usize;
        let payload_start = offset + 1 + reader.current_position();
        let end = payload_start
            .checked_add(size)
            .filter(|end| *end <= data.len())
            .context("section extends beyond binary")?;
        sections.push(RawSection {
            id,
            whole: offset..end,
            payload: payload_start..end,
        });
        offset = end;
    }
    Ok(sections)
}

pub fn custom_section_name(payload: &[u8]) -> anyhow::Result<&str> {
    BinaryReader::new(payload, 0)
        .read_string()
        .map_err(|error| anyhow!("invalid custom section name: {error}"))
}

/// One row of the section table. Rows do not overlap and, in order, cover every byte of the
/// file: the header of each nested module, its sections, and the framing (section id and length)
/// that embeds a nested module or component in its parent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SectionRow {
    pub path: String,
    pub section: String,
    pub offset: u64,
    pub bytes: u64,
}

pub fn section_table(data: &[u8]) -> anyhow::Result<Vec<SectionRow>> {
    let mut rows = Vec::new();
    inspect(data, "component", 0, &mut rows)?;
    Ok(rows)
}

fn inspect(
    data: &[u8],
    label: &str,
    base: usize,
    rows: &mut Vec<SectionRow>,
) -> anyhow::Result<()> {
    let kind = binary_kind(data)?;
    rows.push(SectionRow {
        path: label.to_string(),
        section: "header".to_string(),
        offset: base as u64,
        bytes: 8,
    });
    for section in raw_sections(data)? {
        let payload = &data[section.payload.clone()];
        let nested = kind == BinaryKind::Component
            && matches!(
                section.id,
                COMPONENT_CORE_MODULE_SECTION | COMPONENT_COMPONENT_SECTION
            );
        if nested {
            let child = format!("{label}/{}", rows.len());
            rows.push(SectionRow {
                path: child.clone(),
                section: "framing".to_string(),
                offset: (base + section.whole.start) as u64,
                bytes: (section.payload.start - section.whole.start) as u64,
            });
            inspect(payload, &child, base + section.payload.start, rows)?;
        } else {
            let name = if section.id == CUSTOM_SECTION {
                format!("custom:{}", custom_section_name(payload)?)
            } else {
                match kind {
                    BinaryKind::Core => core_section_name(section.id),
                    BinaryKind::Component => format!("component-{}", section.id),
                }
            };
            rows.push(SectionRow {
                path: label.to_string(),
                section: name,
                offset: (base + section.whole.start) as u64,
                bytes: section.whole.len() as u64,
            });
        }
    }
    Ok(())
}

fn core_section_name(id: u8) -> String {
    match id {
        1 => "type",
        2 => "import",
        3 => "function",
        4 => "table",
        5 => "memory",
        6 => "global",
        7 => "export",
        8 => "start",
        9 => "element",
        10 => "code",
        11 => "data",
        12 => "data-count",
        13 => "tag",
        other => return other.to_string(),
    }
    .to_string()
}

/// Size totals of one file. `code`, `data` and `custom` add up those sections, including their
/// framing, across every embedded core module.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SizeSummary {
    pub bytes: u64,
    pub gzip_bytes: u64,
    pub code: u64,
    pub data: u64,
    pub custom: u64,
    pub sha256: String,
}

pub fn summarize(data: &[u8], rows: &[SectionRow]) -> SizeSummary {
    let mut totals: BTreeMap<&str, u64> = BTreeMap::new();
    for row in rows {
        *totals.entry(row.section.as_str()).or_default() += row.bytes;
    }
    SizeSummary {
        bytes: data.len() as u64,
        gzip_bytes: gzip_size(data),
        code: totals.get("code").copied().unwrap_or_default(),
        data: totals.get("data").copied().unwrap_or_default(),
        custom: totals
            .iter()
            .filter(|(name, _)| name.starts_with("custom:"))
            .map(|(_, bytes)| bytes)
            .sum(),
        sha256: hex::encode(Sha256::digest(data)),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use test_r::test;

    pub fn leb(mut value: usize) -> Vec<u8> {
        let mut out = Vec::new();
        loop {
            let byte = (value & 0x7f) as u8;
            value >>= 7;
            if value == 0 {
                out.push(byte);
                return out;
            }
            out.push(byte | 0x80);
        }
    }

    pub fn section(id: u8, payload: &[u8]) -> Vec<u8> {
        [vec![id], leb(payload.len()), payload.to_vec()].concat()
    }

    pub fn custom(name: &str, payload: &[u8]) -> Vec<u8> {
        section(
            CUSTOM_SECTION,
            &[leb(name.len()), name.as_bytes().to_vec(), payload.to_vec()].concat(),
        )
    }

    pub fn core(sections: &[Vec<u8>]) -> Vec<u8> {
        [CORE_HEADER.to_vec(), sections.concat()].concat()
    }

    pub fn component(sections: &[Vec<u8>]) -> Vec<u8> {
        [COMPONENT_HEADER.to_vec(), sections.concat()].concat()
    }

    #[test]
    fn rows_cover_every_byte_of_nested_modules_exactly_once() {
        let code = section(CORE_CODE_SECTION, &[1, 2, 0, 0x0b]);
        let data = section(CORE_DATA_SECTION, &[0]);
        // A payload over 127 bytes makes the nested length prefix two bytes long.
        let module = core(&[code, data.clone(), custom("name", &[b'x'; 130])]);
        let nested = component(&[section(COMPONENT_CORE_MODULE_SECTION, &module)]);
        let file = component(&[
            section(COMPONENT_COMPONENT_SECTION, &nested),
            section(COMPONENT_CORE_MODULE_SECTION, &core(&[data])),
            custom("component-name", b"wrapper"),
        ]);

        let rows = section_table(&file).unwrap();

        assert_eq!(
            rows.iter().map(|row| row.bytes).sum::<u64>(),
            file.len() as u64
        );
        for pair in rows.windows(2) {
            assert_eq!(pair[0].offset + pair[0].bytes, pair[1].offset);
        }
        let summary = summarize(&file, &rows);
        assert_eq!(summary.code, 6);
        assert_eq!(summary.data, 6);
        // `name`: id, 2-byte length, name length byte, 4-byte name and 130 bytes of payload.
        // `component-name`: id, length, name length byte, 14-byte name and 7 bytes of payload.
        assert_eq!(summary.custom, (1 + 2 + 1 + 4 + 130) + (1 + 1 + 1 + 14 + 7));
        let framing: Vec<_> = rows
            .iter()
            .filter(|row| row.section == "framing")
            .map(|row| row.bytes)
            .collect();
        assert_eq!(framing, vec![3, 3, 2]);
    }

    #[test]
    fn truncated_sections_and_lengths_are_rejected() {
        assert!(raw_sections(&core(&[vec![0, 0x7f, b'a', b'b', b'c']])).is_err());
        assert!(raw_sections(&core(&[vec![1, 0x80]])).is_err());
        assert!(raw_sections(b"\0asm\x02\0\0\0").is_err());
    }
}
