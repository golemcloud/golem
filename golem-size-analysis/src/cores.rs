use crate::sections::{
    BinaryKind, COMPONENT_COMPONENT_SECTION, COMPONENT_CORE_MODULE_SECTION, CORE_HEADER,
    CUSTOM_SECTION, binary_kind, custom_section_name, raw_sections,
};

/// Every core module of a core module or component, in traversal order: depth first, in section
/// order, including the cores of nested components. A core module is its own only core.
pub fn core_modules(data: &[u8]) -> anyhow::Result<Vec<Vec<u8>>> {
    let mut cores = Vec::new();
    rewrite_cores(data, &mut |core| {
        cores.push(core.to_vec());
        Ok(core.to_vec())
    })?;
    Ok(cores)
}

/// Replaces each embedded core module with `transform(core)`, in [`core_modules`] order. Every
/// other byte of the wrapper (types, aliases, instances, canonical ABI wiring and custom metadata)
/// stays as it is; only the length prefixes of the replaced sections change.
pub fn rewrite_cores(
    data: &[u8],
    transform: &mut dyn FnMut(&[u8]) -> anyhow::Result<Vec<u8>>,
) -> anyhow::Result<Vec<u8>> {
    if binary_kind(data)? == BinaryKind::Core {
        return transform(data);
    }
    let mut result = data[..8].to_vec();
    for section in raw_sections(data)? {
        if matches!(
            section.id,
            COMPONENT_CORE_MODULE_SECTION | COMPONENT_COMPONENT_SECTION
        ) {
            let replacement = rewrite_cores(&data[section.payload], transform)?;
            result.push(section.id);
            write_u32_leb(&mut result, replacement.len());
            result.extend_from_slice(&replacement);
        } else {
            result.extend_from_slice(&data[section.whole]);
        }
    }
    Ok(result)
}

/// Restores the `component-type*` custom sections of `original` after a core-module optimizer
/// rewrote the core: `optimized` without its own copies, followed by the original sections
/// byte for byte. `wasm-tools component new` needs them to encode the world of a raw core.
pub fn preserve_component_types(original: &[u8], optimized: &[u8]) -> anyhow::Result<Vec<u8>> {
    let is_type = |data: &[u8], id: u8, payload: std::ops::Range<usize>| -> anyhow::Result<bool> {
        Ok(id == CUSTOM_SECTION
            && custom_section_name(&data[payload])?.starts_with("component-type"))
    };
    let mut result = CORE_HEADER.to_vec();
    for section in raw_sections(optimized)? {
        if !is_type(optimized, section.id, section.payload.clone())? {
            result.extend_from_slice(&optimized[section.whole]);
        }
    }
    for section in raw_sections(original)? {
        if is_type(original, section.id, section.payload.clone())? {
            result.extend_from_slice(&original[section.whole]);
        }
    }
    Ok(result)
}

fn write_u32_leb(out: &mut Vec<u8>, mut value: usize) {
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sections::tests::{component, core, custom, section};
    use test_r::test;

    #[test]
    fn rewrite_updates_nested_lengths_and_keeps_wrapper_and_core_order() {
        let first = core(&[custom("first", b"a")]);
        let second = core(&[custom("second", &[b'b'; 128])]);
        let metadata = custom("component-name", b"wrapper");
        let nested = component(&[
            section(COMPONENT_CORE_MODULE_SECTION, &second),
            metadata.clone(),
        ]);
        let original = component(&[
            section(COMPONENT_CORE_MODULE_SECTION, &first),
            section(COMPONENT_COMPONENT_SECTION, &nested),
            metadata.clone(),
        ]);

        assert_eq!(
            rewrite_cores(&original, &mut |core| Ok(core.to_vec())).unwrap(),
            original
        );
        assert_eq!(core_modules(&original).unwrap(), vec![first, second]);

        let shrunk = rewrite_cores(&original, &mut |_| Ok(CORE_HEADER.to_vec())).unwrap();
        let expected = component(&[
            section(COMPONENT_CORE_MODULE_SECTION, &CORE_HEADER),
            section(
                COMPONENT_COMPONENT_SECTION,
                &component(&[
                    section(COMPONENT_CORE_MODULE_SECTION, &CORE_HEADER),
                    metadata.clone(),
                ]),
            ),
            metadata,
        ]);
        assert_eq!(shrunk, expected);
    }

    #[test]
    fn component_types_are_restored_once_without_stale_names() {
        let types = [
            custom("component-type:a", &[b'a'; 137]),
            custom("component-type:b", b"b"),
        ]
        .concat();
        let original = core(&[types.clone(), custom("name", b"old")]);
        let optimized = core(&[
            custom("component-type:a", b"changed"),
            custom("name", b"new"),
        ]);

        let restored = preserve_component_types(&original, &optimized).unwrap();

        assert_eq!(restored, core(&[custom("name", b"new"), types]));
        assert_eq!(
            preserve_component_types(&original, &restored).unwrap(),
            restored
        );
    }
}
