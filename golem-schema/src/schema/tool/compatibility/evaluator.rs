// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.

use super::*;

/// Applies a compiled directional projection, validating both boundary values.
pub fn apply_projection(
    plan: &ProjectionPlan,
    value: SchemaValue,
    streams: &mut impl ProjectionStreamHandler,
) -> Result<SchemaValue, ToolCompatibilityError> {
    if let Err(errors) = validate_value(&plan.source_schema, &plan.source_schema.root, &value) {
        let error = err(
            "source",
            format!("value does not match source schema: {errors:?}"),
        );
        Evaluator { plan, streams }.discard_value(value);
        return Err(error);
    }
    let mut evaluator = Evaluator { plan, streams };
    let result = evaluator.apply(
        plan.root,
        &plan.source_schema.root,
        &plan.target_schema.root,
        value,
    )?;
    if let Err(errors) = validate_value(&plan.target_schema, &plan.target_schema.root, &result) {
        let error = err(
            "target",
            format!("projected value does not match target schema: {errors:?}"),
        );
        evaluator.discard_value(result);
        return Err(error);
    }
    Ok(result)
}

struct Evaluator<'a, H> {
    plan: &'a ProjectionPlan,
    streams: &'a mut H,
}

impl<H: ProjectionStreamHandler> Evaluator<'_, H> {
    fn apply(
        &mut self,
        node: usize,
        st: &SchemaType,
        tt: &SchemaType,
        value: SchemaValue,
    ) -> Result<SchemaValue, ToolCompatibilityError> {
        let Some(projection) = self.plan.nodes.get(node).cloned() else {
            self.discard_value(value);
            return Err(err("plan", format!("invalid projection node {node}")));
        };
        let (st, _) = resolve(&self.plan.source_schema, st);
        let (tt, _) = resolve(&self.plan.target_schema, tt);
        let bad = || {
            err(
                "value",
                format!("value shape does not match projection node {node}"),
            )
        };
        let shapes_match = match &projection {
            ProjectionNode::Identity
            | ProjectionNode::DynamicChecked
            | ProjectionNode::Recursive { .. } => true,
            ProjectionNode::Record { .. } => {
                matches!(
                    (st, tt),
                    (SchemaType::Record { .. }, SchemaType::Record { .. })
                )
            }
            ProjectionNode::Tuple { .. } => {
                matches!(
                    (st, tt),
                    (SchemaType::Tuple { .. }, SchemaType::Tuple { .. })
                )
            }
            ProjectionNode::List { .. } => {
                matches!((st, tt), (SchemaType::List { .. }, SchemaType::List { .. }))
            }
            ProjectionNode::FixedList { .. } => matches!(
                (st, tt),
                (SchemaType::FixedList { .. }, SchemaType::FixedList { .. })
            ),
            ProjectionNode::Map { .. } => {
                matches!((st, tt), (SchemaType::Map { .. }, SchemaType::Map { .. }))
            }
            ProjectionNode::Option { .. } => {
                matches!(
                    (st, tt),
                    (SchemaType::Option { .. }, SchemaType::Option { .. })
                )
            }
            ProjectionNode::Result { .. } => {
                matches!(
                    (st, tt),
                    (SchemaType::Result { .. }, SchemaType::Result { .. })
                )
            }
            ProjectionNode::Variant { .. } => matches!(
                (st, tt),
                (SchemaType::Variant { .. }, SchemaType::Variant { .. })
            ),
            ProjectionNode::Enum { .. } => {
                matches!((st, tt), (SchemaType::Enum { .. }, SchemaType::Enum { .. }))
            }
            ProjectionNode::Flags { .. } => {
                matches!(
                    (st, tt),
                    (SchemaType::Flags { .. }, SchemaType::Flags { .. })
                )
            }
            ProjectionNode::Union { .. } => {
                matches!(
                    (st, tt),
                    (SchemaType::Union { .. }, SchemaType::Union { .. })
                )
            }
            ProjectionNode::Stream { item } => matches!(
                (item, st, tt),
                (
                    Some(_),
                    SchemaType::Stream { inner: Some(_), .. },
                    SchemaType::Stream { inner: Some(_), .. },
                ) | (None, SchemaType::Stream { .. }, SchemaType::Stream { .. })
            ),
        };
        if !shapes_match {
            self.discard_value(value);
            return Err(err("plan", "projection node does not match its schemas"));
        }
        match projection {
            ProjectionNode::Identity => Ok(value),
            ProjectionNode::DynamicChecked => {
                if let Err(errors) = validate_value(&self.plan.target_schema, tt, &value) {
                    let error = err(
                        "value",
                        format!("dynamic value does not match target subtree: {errors:?}"),
                    );
                    self.discard_value(value);
                    return Err(error);
                }
                Ok(value)
            }
            ProjectionNode::Recursive { node } => self.apply(node, st, tt, value),
            ProjectionNode::Record { fields, discard } => {
                let (SchemaType::Record { fields: sf, .. }, SchemaType::Record { fields: tf, .. }) =
                    (st, tt)
                else {
                    self.discard_value(value);
                    return Err(bad());
                };
                let SchemaValue::Record { fields: values } = value else {
                    self.discard_value(value);
                    return Err(bad());
                };
                let mut values = values.into_iter().map(Some).collect::<Vec<_>>();
                let mut claimed = HashSet::new();
                let mut discarded = HashSet::new();
                if fields.len() != tf.len()
                    || discard.iter().any(|&index| index >= values.len())
                    || discard.iter().any(|&index| !discarded.insert(index))
                    || fields.iter().enumerate().any(|(target_index, field)| {
                        target_index >= tf.len()
                            || match field.source_index {
                                Some(index) => {
                                    index >= values.len()
                                        || field.plan.is_none()
                                        || field.default.is_some()
                                        || !claimed.insert(index)
                                }
                                None => field.plan.is_some() || field.default.is_none(),
                            }
                    })
                    || discard.iter().any(|index| claimed.contains(index))
                    || claimed.len() + discarded.len() != values.len()
                    || sf.len() != values.len()
                {
                    self.discard_remaining(values);
                    return Err(err("plan", "invalid record projection"));
                }
                for index in discard {
                    self.discard_value(values[index].take().unwrap());
                }
                let mut output = Vec::with_capacity(fields.len());
                for (target_index, field) in fields.into_iter().enumerate() {
                    let value = match (field.source_index, field.plan, field.default) {
                        (Some(source_index), Some(child), None) => match self.apply(
                            child,
                            &sf[source_index].body,
                            &tf[target_index].body,
                            values[source_index].take().unwrap(),
                        ) {
                            Ok(value) => value,
                            Err(error) => {
                                self.discard_remaining(output);
                                self.discard_remaining(values);
                                return Err(error);
                            }
                        },
                        (None, None, Some(default)) => default,
                        _ => unreachable!("record projection was validated"),
                    };
                    output.push(value);
                }
                Ok(SchemaValue::Record { fields: output })
            }
            ProjectionNode::Tuple { elements } => {
                let (
                    SchemaType::Tuple { elements: ss, .. },
                    SchemaType::Tuple { elements: ts, .. },
                    SchemaValue::Tuple { elements: values },
                ) = (st, tt, value)
                else {
                    return Err(bad());
                };
                Ok(SchemaValue::Tuple {
                    elements: self.sequence(elements, ss, ts, values)?,
                })
            }
            ProjectionNode::List { item } | ProjectionNode::FixedList { item, .. } => {
                let (ss, ts, values, fixed) = match (st, tt, value) {
                    (
                        SchemaType::List { element: ss, .. },
                        SchemaType::List { element: ts, .. },
                        SchemaValue::List { elements },
                    ) => (ss.as_ref(), ts.as_ref(), elements, false),
                    (
                        SchemaType::FixedList { element: ss, .. },
                        SchemaType::FixedList { element: ts, .. },
                        SchemaValue::FixedList { elements },
                    ) => (ss.as_ref(), ts.as_ref(), elements, true),
                    _ => return Err(bad()),
                };
                let output = self.project_sequence(
                    std::iter::repeat_n(item, values.len()).collect(),
                    std::iter::repeat_n(ss, values.len()).collect(),
                    std::iter::repeat_n(ts, values.len()).collect(),
                    values,
                )?;
                Ok(if fixed {
                    SchemaValue::FixedList { elements: output }
                } else {
                    SchemaValue::List { elements: output }
                })
            }
            ProjectionNode::Map { key, value: vp } => {
                let (
                    SchemaType::Map {
                        key: sk, value: sv, ..
                    },
                    SchemaType::Map {
                        key: tk, value: tv, ..
                    },
                    SchemaValue::Map { entries },
                ) = (st, tt, value)
                else {
                    return Err(bad());
                };
                let mut remaining = entries.into_iter().map(Some).collect::<Vec<_>>();
                let mut output = Vec::with_capacity(remaining.len());
                for i in 0..remaining.len() {
                    let (k, v) = remaining[i].take().unwrap();
                    let projected_key = match self.apply(key, sk, tk, k) {
                        Ok(key) => key,
                        Err(error) => {
                            self.discard_value(v);
                            self.discard_map_entries(output);
                            self.discard_map_entries(remaining.into_iter().flatten());
                            return Err(error);
                        }
                    };
                    match self.apply(vp, sv, tv, v) {
                        Ok(value) => output.push((projected_key, value)),
                        Err(error) => {
                            self.discard_value(projected_key);
                            self.discard_map_entries(output);
                            self.discard_map_entries(remaining.into_iter().flatten());
                            return Err(error);
                        }
                    }
                }
                Ok(SchemaValue::Map { entries: output })
            }
            ProjectionNode::Option { some } => {
                let (
                    SchemaType::Option { inner: ss, .. },
                    SchemaType::Option { inner: ts, .. },
                    SchemaValue::Option { inner },
                ) = (st, tt, value)
                else {
                    return Err(bad());
                };
                Ok(SchemaValue::Option {
                    inner: inner
                        .map(|v| self.apply(some, ss, ts, *v).map(Box::new))
                        .transpose()?,
                })
            }
            ProjectionNode::Result { ok, err: ep } => {
                let (
                    SchemaType::Result { spec: ss, .. },
                    SchemaType::Result { spec: ts, .. },
                    SchemaValue::Result(payload),
                ) = (st, tt, value)
                else {
                    return Err(bad());
                };
                let payload = match payload {
                    ResultValuePayload::Ok { value } => ResultValuePayload::Ok {
                        value: self.optional_payload(
                            ok,
                            ss.ok.as_deref(),
                            ts.ok.as_deref(),
                            value,
                        )?,
                    },
                    ResultValuePayload::Err { value } => ResultValuePayload::Err {
                        value: self.optional_payload(
                            ep,
                            ss.err.as_deref(),
                            ts.err.as_deref(),
                            value,
                        )?,
                    },
                };
                Ok(SchemaValue::Result(payload))
            }
            ProjectionNode::Variant { cases } => {
                let (
                    SchemaType::Variant { cases: ss, .. },
                    SchemaType::Variant { cases: ts, .. },
                    SchemaValue::Variant(payload),
                ) = (st, tt, value)
                else {
                    return Err(bad());
                };
                let case = cases
                    .iter()
                    .find(|c| c.source_index == payload.case as usize)
                    .cloned();
                let Some(case) = case else {
                    self.discard_remaining(payload.payload.map(|value| *value));
                    return Err(bad());
                };
                let (Some(source_case), Some(target_case)) =
                    (ss.get(case.source_index), ts.get(case.target_index))
                else {
                    self.discard_remaining(payload.payload.map(|value| *value));
                    return Err(err("plan", "invalid variant case projection"));
                };
                Ok(SchemaValue::Variant(VariantValuePayload {
                    case: case.target_index as u32,
                    payload: self.optional_payload(
                        case.payload,
                        source_case.payload.as_ref(),
                        target_case.payload.as_ref(),
                        payload.payload,
                    )?,
                }))
            }
            ProjectionNode::Union { branches } => {
                let (
                    SchemaType::Union { spec: ss, .. },
                    SchemaType::Union { spec: ts, .. },
                    SchemaValue::Union(payload),
                ) = (st, tt, value)
                else {
                    return Err(bad());
                };
                let case = branches.iter().find(|c| {
                    ss.branches
                        .get(c.source_index)
                        .is_some_and(|branch| branch.tag == payload.tag)
                });
                let Some(case) = case else {
                    self.discard_value(*payload.body);
                    return Err(bad());
                };
                let (Some(source_branch), Some(target_branch)) = (
                    ss.branches.get(case.source_index),
                    ts.branches.get(case.target_index),
                ) else {
                    self.discard_value(*payload.body);
                    return Err(err("plan", "invalid union branch projection"));
                };
                let Some(child) = case.payload else {
                    self.discard_value(*payload.body);
                    return Err(err("plan", "union branch has no payload plan"));
                };
                Ok(SchemaValue::Union(UnionValuePayload {
                    tag: target_branch.tag.clone(),
                    body: Box::new(self.apply(
                        child,
                        &source_branch.body,
                        &target_branch.body,
                        *payload.body,
                    )?),
                }))
            }
            ProjectionNode::Enum { cases } => match value {
                SchemaValue::Enum { case } => Ok(SchemaValue::Enum {
                    case: *cases.get(case as usize).ok_or_else(bad)? as u32,
                }),
                _ => Err(bad()),
            },
            ProjectionNode::Flags { flags } => match value {
                SchemaValue::Flags { bits } => {
                    let target_len = match tt {
                        SchemaType::Flags { flags, .. } => flags.len(),
                        _ => return Err(bad()),
                    };
                    let mut out = vec![false; target_len];
                    for (i, bit) in bits.into_iter().enumerate() {
                        if bit {
                            let Some(&target) = flags.get(i) else {
                                return Err(bad());
                            };
                            let Some(target_bit) = out.get_mut(target) else {
                                return Err(err("plan", "invalid target flag index"));
                            };
                            *target_bit = true;
                        }
                    }
                    Ok(SchemaValue::Flags { bits: out })
                }
                _ => Err(bad()),
            },
            ProjectionNode::Stream { item } => match value {
                SchemaValue::Stream(stream) => {
                    let item_plan = match (item, st, tt) {
                        (
                            Some(root),
                            SchemaType::Stream {
                                inner: Some(ss), ..
                            },
                            SchemaType::Stream {
                                inner: Some(ts), ..
                            },
                        ) => Some(ProjectionPlan {
                            source_schema: graph_for(&self.plan.source_schema, ss),
                            target_schema: graph_for(&self.plan.target_schema, ts),
                            nodes: self.plan.nodes.clone(),
                            root,
                        }),
                        (None, _, _) => {
                            return Ok(SchemaValue::Stream(stream));
                        }
                        _ => {
                            self.streams.discard_stream(stream);
                            return Err(err(
                                "plan",
                                "stream item projection does not match stream schemas",
                            ));
                        }
                    };
                    self.streams
                        .project_stream(stream, item_plan)
                        .map(SchemaValue::Stream)
                        .map_err(|e| err("stream", e))
                }
                _ => Err(bad()),
            },
        }
    }

    fn sequence(
        &mut self,
        plans: Vec<usize>,
        ss: &[SchemaType],
        ts: &[SchemaType],
        values: Vec<SchemaValue>,
    ) -> Result<Vec<SchemaValue>, ToolCompatibilityError> {
        if plans.len() != values.len() || ss.len() != values.len() || ts.len() != values.len() {
            self.discard_remaining(values);
            return Err(err("value", "tuple length does not match projection"));
        }
        self.project_sequence(plans, ss.iter().collect(), ts.iter().collect(), values)
    }
    fn project_sequence(
        &mut self,
        plans: Vec<usize>,
        ss: Vec<&SchemaType>,
        ts: Vec<&SchemaType>,
        values: Vec<SchemaValue>,
    ) -> Result<Vec<SchemaValue>, ToolCompatibilityError> {
        let mut remaining = values.into_iter().map(Some).collect::<Vec<_>>();
        let mut output = Vec::with_capacity(remaining.len());
        for i in 0..remaining.len() {
            match self.apply(plans[i], ss[i], ts[i], remaining[i].take().unwrap()) {
                Ok(value) => output.push(value),
                Err(error) => {
                    self.discard_remaining(output);
                    self.discard_remaining(remaining);
                    return Err(error);
                }
            }
        }
        Ok(output)
    }
    fn discard_remaining<T: IntoIterator<Item = V>, V: IntoRemainingValue>(&mut self, values: T) {
        for value in values {
            if let Some(value) = value.into_remaining_value() {
                self.discard_value(value);
            }
        }
    }
    fn discard_map_entries(
        &mut self,
        entries: impl IntoIterator<Item = (SchemaValue, SchemaValue)>,
    ) {
        for (key, value) in entries {
            self.discard_value(key);
            self.discard_value(value);
        }
    }
    fn optional_payload(
        &mut self,
        plan: Option<usize>,
        ss: Option<&SchemaType>,
        ts: Option<&SchemaType>,
        value: Option<Box<SchemaValue>>,
    ) -> Result<Option<Box<SchemaValue>>, ToolCompatibilityError> {
        match (plan, ss, ts, value) {
            (None, None, None, None) => Ok(None),
            (Some(p), Some(s), Some(t), Some(v)) => self.apply(p, s, t, *v).map(Box::new).map(Some),
            (_, _, _, value) => {
                self.discard_remaining(value.map(|value| *value));
                Err(err("value", "payload presence does not match projection"))
            }
        }
    }
    fn discard_value(&mut self, value: SchemaValue) {
        match value {
            SchemaValue::Stream(stream) => self.streams.discard_stream(stream),
            SchemaValue::Record { fields }
            | SchemaValue::Tuple { elements: fields }
            | SchemaValue::List { elements: fields }
            | SchemaValue::FixedList { elements: fields } => {
                for v in fields {
                    self.discard_value(v);
                }
            }
            SchemaValue::Map { entries } => {
                for (k, v) in entries {
                    self.discard_value(k);
                    self.discard_value(v);
                }
            }
            SchemaValue::Variant(v) => {
                if let Some(value) = v.payload {
                    self.discard_value(*value);
                }
            }
            SchemaValue::Option { inner: Some(value) } => {
                self.discard_value(*value);
            }
            SchemaValue::Result(
                ResultValuePayload::Ok { value } | ResultValuePayload::Err { value },
            ) => {
                if let Some(value) = value {
                    self.discard_value(*value);
                }
            }
            SchemaValue::Union(v) => self.discard_value(*v.body),
            _ => {}
        }
    }
}

trait IntoRemainingValue {
    fn into_remaining_value(self) -> Option<SchemaValue>;
}

impl IntoRemainingValue for SchemaValue {
    fn into_remaining_value(self) -> Option<SchemaValue> {
        Some(self)
    }
}

impl IntoRemainingValue for Option<SchemaValue> {
    fn into_remaining_value(self) -> Option<SchemaValue> {
        self
    }
}
