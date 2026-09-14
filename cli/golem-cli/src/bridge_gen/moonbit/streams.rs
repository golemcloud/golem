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

use super::*;
use std::collections::HashSet;

impl MoonBitBridgeGenerator {
    /// Typed cleanup never runs the fallible encoder. Shared affine holders
    /// make this safe after a sibling was converted but not yet lowered.
    pub(super) fn release_expr(
        &self,
        value: &str,
        typ: &SchemaType,
        depth: usize,
    ) -> anyhow::Result<String> {
        if !contains_stream_in_graph(&self.agent_type.schema, typ) {
            return Ok(format!("ignore({value})"));
        }
        if let Some(name) = self.type_naming.type_name_for_type(typ)
            && is_named_composite(self.resolve_ref(typ))
        {
            return Ok(format!("release_{}({value})", name.name));
        }
        self.release_structural(value, self.resolve_ref(typ), depth)
    }

    fn release_structural(
        &self,
        value: &str,
        typ: &SchemaType,
        depth: usize,
    ) -> anyhow::Result<String> {
        let e = format!("release_item{depth}");
        let next = depth + 1;
        Ok(match typ {
            SchemaType::Stream { .. } => format!("{value}.drop()"),
            SchemaType::Record { fields, .. } => {
                let names = self.record_field_idents(fields);
                let statements = fields
                    .iter()
                    .zip(names)
                    .map(|(f, name)| self.release_expr(&format!("{value}.{name}"), &f.body, next))
                    .collect::<anyhow::Result<Vec<_>>>()?;
                format!("{{ {} }}", statements.join("; "))
            }
            SchemaType::Tuple { elements, .. } => {
                let statements = elements
                    .iter()
                    .enumerate()
                    .map(|(i, t)| {
                        self.release_expr(
                            &if elements.len() == 1 {
                                value.to_string()
                            } else {
                                format!("{value}.{i}")
                            },
                            t,
                            next,
                        )
                    })
                    .collect::<anyhow::Result<Vec<_>>>()?;
                format!("{{ {} }}", statements.join("; "))
            }
            SchemaType::Option { inner, .. } => format!(
                "match {value} {{ Some({e}) => {}; None => () }}",
                self.release_expr(&e, inner, next)?
            ),
            SchemaType::List { element, .. } | SchemaType::FixedList { element, .. } => format!(
                "for {e} in {value} {{ {} }}",
                self.release_expr(&e, element, next)?
            ),
            SchemaType::Map {
                key, value: item, ..
            } => format!(
                "{value}.each((release_key{depth}, {e}) => {{ {}; {} }})",
                self.release_expr(&format!("release_key{depth}"), key, next)?,
                self.release_expr(&e, item, next)?
            ),
            SchemaType::Result { spec, .. } => {
                let ok = spec
                    .ok
                    .as_deref()
                    .map(|t| self.release_expr(&e, t, next))
                    .transpose()?
                    .unwrap_or_else(|| format!("ignore({e})"));
                let err = spec
                    .err
                    .as_deref()
                    .map(|t| self.release_expr(&e, t, next))
                    .transpose()?
                    .unwrap_or_else(|| format!("ignore({e})"));
                format!("match {value} {{ Ok({e}) => {ok}; Err({e}) => {err} }}")
            }
            SchemaType::Variant { cases, .. } => {
                let names = self.variant_case_idents(cases.iter().map(|c| c.name.as_str()));
                let arms = cases
                    .iter()
                    .zip(names)
                    .map(|(case, name)| {
                        Ok(match &case.payload {
                            Some(t) => {
                                format!("{name}({e}) => {}", self.release_expr(&e, t, next)?)
                            }
                            None => format!("{name} => ()"),
                        })
                    })
                    .collect::<anyhow::Result<Vec<_>>>()?;
                format!("match {value} {{ {} }}", arms.join("; "))
            }
            SchemaType::Union { spec, .. } => {
                let names = self.variant_case_idents(spec.branches.iter().map(|b| b.tag.as_str()));
                let arms = spec
                    .branches
                    .iter()
                    .zip(names)
                    .map(|(b, name)| {
                        Ok(format!(
                            "{name}({e}) => {}",
                            self.release_expr(&e, &b.body, next)?
                        ))
                    })
                    .collect::<anyhow::Result<Vec<_>>>()?;
                format!("match {value} {{ {} }}", arms.join("; "))
            }
            _ => format!("ignore({value})"),
        })
    }

    pub(super) fn write_guest_stream_support(
        &self,
        writer: &mut MoonBitWriter,
    ) -> anyhow::Result<()> {
        for (typ, name) in self.type_naming.types() {
            let resolved = self.resolve_ref(typ);
            if is_named_composite(resolved)
                && contains_stream_in_graph(&self.agent_type.schema, typ)
            {
                writer.line(format!(
                    "fn release_{}(value : {}) -> Unit {{ {} }}",
                    name.name,
                    name.name,
                    self.release_structural("value", resolved, 0)?
                ));
                writer.blank();
            }
        }
        let bases = self.method_base_idents(&self.agent_type.methods);
        for (method, base) in self.agent_type.methods.iter().zip(bases) {
            for (i, field) in user_supplied_fields(&method.input_schema)
                .iter()
                .enumerate()
            {
                self.write_stream_factories(
                    writer,
                    &field.schema,
                    &format!("{base}_input_{i}"),
                    &mut HashSet::new(),
                )?;
            }
            if let OutputSchema::Single(output) = &method.output_schema {
                self.write_stream_factories(
                    writer,
                    output,
                    &format!("{base}_output"),
                    &mut HashSet::new(),
                )?;
            }
        }
        Ok(())
    }

    fn write_stream_factories(
        &self,
        writer: &mut MoonBitWriter,
        typ: &SchemaType,
        path: &str,
        visiting: &mut HashSet<String>,
    ) -> anyhow::Result<()> {
        if let SchemaType::Ref { id, .. } = typ {
            if !visiting.insert(id.to_string()) {
                return Ok(());
            }
            self.write_stream_factories(writer, self.resolve_ref(typ), path, visiting)?;
            visiting.remove(&id.to_string());
            return Ok(());
        }
        if let SchemaType::Stream { inner, .. } = typ {
            let inner = inner
                .as_deref()
                .context("MoonBit guest streams require an element schema")?;
            let ty = self.type_reference(inner)?;
            let encode = guest_codec_source(self.encode_expr("item", inner, 0)?);
            let decode = guest_codec_source(self.decode_expr("item", inner, 0)?);
            let release = self.release_expr("item", inner, 0)?;
            writer.line("#warnings(\"-unused_error_type\")");
            writer.line(format!(
                "fn stream_encode_{path}(item : {ty}) -> @model.SchemaValue raise {{ {encode} }}"
            ));
            writer.blank();
            writer.line("#warnings(\"-unused_try\")");
            writer.line(format!("fn stream_decode_{path}(item : @model.SchemaValue) -> {ty} raise @schema.FromSchemaError {{ {decode} catch {{ error => raise @schema.FromSchemaError::custom(repr(error)) }} }}"));
            writer.blank();
            writer.doc(&format!(
                "Creates native endpoints for the {path} stream schema occurrence."
            ));
            writer.line(format!("pub fn new_{path}_stream() -> (@schema.AgentStreamWriter[{ty}], @schema.AgentStream[{ty}]) {{ @schema.AgentStream::new_with_codec(stream_encode_{path}, stream_decode_{path}, item => {{ {release} }}) }}"));
            writer.blank();
            writer.doc(&format!("Creates a lazy native producer using the exact {path} item schema, not the MoonBit type's default schema. Writes consume their items, including on conversion failure or peer drop. Use on_unstarted_drop to release owned captures if the stream is dropped before the producer starts."));
            writer.line(format!("pub fn produce_{path}_stream(producer : async (@schema.AgentStreamWriter[{ty}]) -> Unit, on_unstarted_drop? : () -> Unit) -> @schema.AgentStream[{ty}] {{ @schema.AgentStream::produce_with_codec(producer, stream_encode_{path}, stream_decode_{path}, item => {{ {release} }}, on_unstarted_drop?) }}"));
            writer.blank();
        }
        let children: Vec<&SchemaType> = match typ {
            SchemaType::Stream { inner, .. } | SchemaType::Future { inner, .. } => {
                inner.iter().map(|t| t.as_ref()).collect()
            }
            SchemaType::Record { fields, .. } => fields.iter().map(|f| &f.body).collect(),
            SchemaType::Variant { cases, .. } => {
                cases.iter().filter_map(|c| c.payload.as_ref()).collect()
            }
            SchemaType::Union { spec, .. } => spec.branches.iter().map(|b| &b.body).collect(),
            SchemaType::Tuple { elements, .. } => elements.iter().collect(),
            SchemaType::List { element, .. } | SchemaType::FixedList { element, .. } => {
                vec![element]
            }
            SchemaType::Option { inner, .. } => vec![inner],
            SchemaType::Map { key, value, .. } => vec![key, value],
            SchemaType::Result { spec, .. } => spec
                .ok
                .iter()
                .chain(spec.err.iter())
                .map(|t| t.as_ref())
                .collect(),
            _ => vec![],
        };
        for (i, child) in children.into_iter().enumerate() {
            self.write_stream_factories(writer, child, &format!("{path}_{i}"), visiting)?;
        }
        Ok(())
    }
}
