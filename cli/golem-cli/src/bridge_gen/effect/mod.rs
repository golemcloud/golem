// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1

pub mod effect_external;
pub mod effect_guest;
pub mod effect_tool;

use crate::bridge_gen::parameter_naming::ParameterNaming;

pub(crate) fn effect_stream_alias(agent_name: &str) -> String {
    let mut naming = ParameterNaming::new();
    naming.reserve(agent_name);
    naming.fresh("EffectStream")
}

/// Maps TypeScript SDK stream references to Effect's native stream type.
pub(crate) fn effect_type_reference(source: &str, stream_alias: &str) -> String {
    const SOURCE: &str = "base.AgentStream<";
    let mut result = String::with_capacity(source.len());
    let mut remaining = source;
    while let Some(start) = remaining.find(SOURCE) {
        result.push_str(&remaining[..start]);
        result.push_str(stream_alias);
        result.push_str(".Stream<");
        remaining = &remaining[start + SOURCE.len()..];
        let mut depth = 1;
        let mut end = None;
        for (index, ch) in remaining.char_indices() {
            match ch {
                '<' => depth += 1,
                '>' => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(index);
                        break;
                    }
                }
                _ => {}
            }
        }
        let Some(end) = end else {
            result.push_str(remaining);
            return result;
        };
        result.push_str(&effect_type_reference(&remaining[..end], stream_alias));
        result.push_str(", unknown>");
        remaining = &remaining[end + 1..];
    }
    result.push_str(remaining);
    result
}
