// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at http://license.golem.cloud/LICENSE

use super::canonical::{CanonicalInputValue, CanonicalSurfaceRef};
use super::{Constraint, FlagShape, Quantifier, Ref, Tool};
use crate::schema::schema_value::SchemaValue;

fn ref_matches(
    reference: &Ref,
    tool: &Tool,
    command_index: usize,
    surfaces: &[CanonicalSurfaceRef],
    values: &[CanonicalInputValue],
) -> bool {
    let (name, expected) = match reference {
        Ref::Present(name) => (name, None),
        Ref::ValueIs(value) => (&value.name, Some(&value.value)),
    };
    let Some((index, value)) = values
        .iter()
        .enumerate()
        .find(|(_, value)| value.name == *name || value.aliases.contains(name))
    else {
        return false;
    };
    if let Some(expected) = expected {
        return schema_value_matches(&value.value, expected);
    }
    surface_is_present(tool, command_index, surfaces[index], &value.value)
}

fn schema_value_matches(value: &SchemaValue, expected: &SchemaValue) -> bool {
    if value == expected {
        return true;
    }
    match value {
        SchemaValue::Option { inner } => inner
            .as_deref()
            .is_some_and(|value| schema_value_matches(value, expected)),
        SchemaValue::List { elements } | SchemaValue::FixedList { elements } => elements
            .iter()
            .any(|value| schema_value_matches(value, expected)),
        SchemaValue::Map { entries } => entries
            .iter()
            .any(|(_, value)| schema_value_matches(value, expected)),
        _ => false,
    }
}

fn value_is_present(value: &SchemaValue, default: Option<&SchemaValue>) -> bool {
    if default.is_some_and(|default| value == default) {
        return false;
    }
    match value {
        SchemaValue::Option { inner } => inner.is_some(),
        SchemaValue::List { elements } | SchemaValue::FixedList { elements } => {
            !elements.is_empty()
        }
        SchemaValue::Map { entries } => !entries.is_empty(),
        SchemaValue::Bool(value) => *value,
        SchemaValue::U32(value) => *value != 0,
        _ => true,
    }
}

fn surface_is_present(
    tool: &Tool,
    command_index: usize,
    surface: CanonicalSurfaceRef,
    value: &SchemaValue,
) -> bool {
    let body = || {
        tool.commands.nodes[command_index]
            .body
            .as_ref()
            .expect("canonical input surfaces only resolve command bodies")
    };
    match surface {
        CanonicalSurfaceRef::GlobalOption { node, index } => value_is_present(
            value,
            tool.commands.nodes[node].globals.options[index]
                .default
                .as_ref(),
        ),
        CanonicalSurfaceRef::BodyOption { index } => {
            value_is_present(value, body().options[index].default.as_ref())
        }
        CanonicalSurfaceRef::GlobalFlag { node, index } => {
            flag_is_present(&tool.commands.nodes[node].globals.flags[index].shape, value)
        }
        CanonicalSurfaceRef::BodyFlag { index } => {
            flag_is_present(&body().flags[index].shape, value)
        }
        CanonicalSurfaceRef::BodyPositional { index } => {
            value_is_present(value, body().positionals.fixed[index].default.as_ref())
        }
        CanonicalSurfaceRef::BodyTail => value_is_present(value, None),
    }
}

fn flag_is_present(shape: &FlagShape, value: &SchemaValue) -> bool {
    match (shape, value) {
        (FlagShape::BoolFlag(shape), SchemaValue::Bool(value)) => *value != shape.default,
        (FlagShape::CountFlag(_), SchemaValue::U32(value)) => *value != 0,
        _ => false,
    }
}

fn quantified_refs(
    quantifier: Quantifier,
    refs: &[Ref],
    tool: &Tool,
    command_index: usize,
    surfaces: &[CanonicalSurfaceRef],
    values: &[CanonicalInputValue],
) -> bool {
    match quantifier {
        Quantifier::All => refs
            .iter()
            .all(|reference| ref_matches(reference, tool, command_index, surfaces, values)),
        Quantifier::Any => refs
            .iter()
            .any(|reference| ref_matches(reference, tool, command_index, surfaces, values)),
    }
}

pub fn validate_tool_constraints(
    tool: &Tool,
    command_index: usize,
    constraints: &[Constraint],
    surfaces: &[CanonicalSurfaceRef],
    values: &[CanonicalInputValue],
) -> Result<(), String> {
    for (index, constraint) in constraints.iter().enumerate() {
        let satisfied = match constraint {
            Constraint::RequiresAll(refs) => {
                quantified_refs(Quantifier::All, refs, tool, command_index, surfaces, values)
            }
            Constraint::AllOrNone(refs) => {
                let present = refs
                    .iter()
                    .filter(|reference| {
                        ref_matches(reference, tool, command_index, surfaces, values)
                    })
                    .count();
                present == 0 || present == refs.len()
            }
            Constraint::RequiresAny(refs) => {
                quantified_refs(Quantifier::Any, refs, tool, command_index, surfaces, values)
            }
            Constraint::MutexGroups(groups) => {
                groups
                    .iter()
                    .filter(|group| {
                        quantified_refs(
                            Quantifier::All,
                            &group.refs,
                            tool,
                            command_index,
                            surfaces,
                            values,
                        )
                    })
                    .count()
                    <= 1
            }
            Constraint::Implies(implies) => {
                !quantified_refs(
                    implies.lhs_quant,
                    &implies.lhs,
                    tool,
                    command_index,
                    surfaces,
                    values,
                ) || quantified_refs(
                    implies.rhs_quant,
                    &implies.rhs,
                    tool,
                    command_index,
                    surfaces,
                    values,
                )
            }
            Constraint::Forbids(forbids) => {
                !quantified_refs(
                    forbids.lhs_quant,
                    &forbids.lhs,
                    tool,
                    command_index,
                    surfaces,
                    values,
                ) || !quantified_refs(
                    Quantifier::Any,
                    &forbids.rhs,
                    tool,
                    command_index,
                    surfaces,
                    values,
                )
            }
        };
        if !satisfied {
            return Err(format!("tool command constraint {index} is not satisfied"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{schema_value_matches, value_is_present};
    use crate::schema::schema_value::SchemaValue;
    use test_r::test;

    #[test]
    fn value_is_matches_nested_optional_and_repeated_values() {
        let expected = SchemaValue::String("selected".to_string());
        assert!(schema_value_matches(
            &SchemaValue::Option {
                inner: Some(Box::new(expected.clone()))
            },
            &expected
        ));
        assert!(schema_value_matches(
            &SchemaValue::List {
                elements: vec![expected.clone()]
            },
            &expected
        ));
    }

    #[test]
    fn defaults_and_empty_collections_are_not_present() {
        let default = SchemaValue::String("default".to_string());
        assert!(!value_is_present(&default, Some(&default)));
        assert!(!value_is_present(
            &SchemaValue::List {
                elements: Vec::new()
            },
            None
        ));
        assert!(!value_is_present(
            &SchemaValue::Option { inner: None },
            None
        ));
    }
}
