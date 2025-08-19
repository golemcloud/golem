// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// Builder for WitType, to be eventually upstreamed to `golem_wasm_rpc`

use golem_wasm_rpc::golem_rpc_0_2_x::types::{NamedWitTypeNode, ResourceId};
use golem_wasm_rpc::{NodeIndex, ResourceMode, WitType, WitTypeNode};

pub trait WitTypeBuilderExtensions {
    fn builder() -> WitTypeBuilder;
}

impl WitTypeBuilderExtensions for WitType {
    fn builder() -> WitTypeBuilder {
        WitTypeBuilder::new()
    }
}

pub trait TypeNodeBuilder: Sized {
    type Result;

    fn parent_builder(&mut self) -> &mut WitTypeBuilder;

    fn u8(self) -> Self::Result;
    fn u16(self) -> Self::Result;
    fn u32(self) -> Self::Result;
    fn u64(self) -> Self::Result;
    fn s8(self) -> Self::Result;
    fn s16(self) -> Self::Result;
    fn s32(self) -> Self::Result;
    fn s64(self) -> Self::Result;
    fn f32(self) -> Self::Result;
    fn f64(self) -> Self::Result;
    fn string(self) -> Self::Result;
    fn bool(self) -> Self::Result;
    fn char(self) -> Self::Result;
    fn option(self, name: Option<String>, owner: Option<String>) -> WitTypeContainerBuilder<Self>;
    fn list(self, name: Option<String>, owner: Option<String>) -> WitTypeContainerBuilder<Self>;
    fn r#enum(self, name: Option<String>, owner: Option<String>, values: &[&str]) -> Self::Result;
    fn flags(self, name: Option<String>, owner: Option<String>, values: &[&str]) -> Self::Result;
    fn record(self, name: Option<String>, owner: Option<String>) -> WitTypeRecordBuilder<Self>;
    fn tuple(self, name: Option<String>, owner: Option<String>) -> WitTypeTupleBuilder<Self>;
    fn variant(self, name: Option<String>, owner: Option<String>) -> WitTypeVariantBuilder<Self>;
    fn result(self, name: Option<String>, owner: Option<String>) -> WitTypeResultBuilder<Self>;
    fn handle(
        self,
        name: Option<String>,
        owner: Option<String>,
        resource_id: ResourceId,
        resource_mode: ResourceMode,
    ) -> Self::Result;

    fn finish(self) -> Self::Result;
}

pub struct WitTypeBuilder {
    nodes: Vec<NamedWitTypeNode>,
}

impl WitTypeBuilder {
    pub(crate) fn new() -> Self {
        WitTypeBuilder { nodes: Vec::new() }
    }

    fn add(&mut self, node: NamedWitTypeNode) -> NodeIndex {
        self.nodes.push(node);
        self.nodes.len() as NodeIndex - 1
    }

    pub(crate) fn add_u8(&mut self) -> NodeIndex {
        self.add(NamedWitTypeNode {
            name: None,
            owner: None,
            type_: WitTypeNode::PrimU8Type,
        })
    }

    pub(crate) fn add_u16(&mut self) -> NodeIndex {
        self.add(NamedWitTypeNode {
            name: None,
            owner: None,
            type_: WitTypeNode::PrimU16Type,
        })
    }

    pub(crate) fn add_u32(&mut self) -> NodeIndex {
        self.add(NamedWitTypeNode {
            name: None,
            owner: None,
            type_: WitTypeNode::PrimU32Type,
        })
    }

    pub(crate) fn add_u64(&mut self) -> NodeIndex {
        self.add(NamedWitTypeNode {
            name: None,
            owner: None,
            type_: WitTypeNode::PrimU64Type,
        })
    }

    pub(crate) fn add_s8(&mut self) -> NodeIndex {
        self.add(NamedWitTypeNode {
            name: None,
            owner: None,
            type_: WitTypeNode::PrimS8Type,
        })
    }

    pub(crate) fn add_s16(&mut self) -> NodeIndex {
        self.add(NamedWitTypeNode {
            name: None,
            owner: None,
            type_: WitTypeNode::PrimS16Type,
        })
    }

    pub(crate) fn add_s32(&mut self) -> NodeIndex {
        self.add(NamedWitTypeNode {
            name: None,
            owner: None,
            type_: WitTypeNode::PrimS32Type,
        })
    }

    pub(crate) fn add_s64(&mut self) -> NodeIndex {
        self.add(NamedWitTypeNode {
            name: None,
            owner: None,
            type_: WitTypeNode::PrimS64Type,
        })
    }

    pub(crate) fn add_f32(&mut self) -> NodeIndex {
        self.add(NamedWitTypeNode {
            name: None,
            owner: None,
            type_: WitTypeNode::PrimF32Type,
        })
    }

    pub(crate) fn add_f64(&mut self) -> NodeIndex {
        self.add(NamedWitTypeNode {
            name: None,
            owner: None,
            type_: WitTypeNode::PrimF64Type,
        })
    }

    pub(crate) fn add_string(&mut self) -> NodeIndex {
        self.add(NamedWitTypeNode {
            name: None,
            owner: None,
            type_: WitTypeNode::PrimStringType,
        })
    }

    pub(crate) fn add_bool(&mut self) -> NodeIndex {
        self.add(NamedWitTypeNode {
            name: None,
            owner: None,
            type_: WitTypeNode::PrimBoolType,
        })
    }

    pub(crate) fn add_char(&mut self) -> NodeIndex {
        self.add(NamedWitTypeNode {
            name: None,
            owner: None,
            type_: WitTypeNode::PrimCharType,
        })
    }

    pub(crate) fn add_record(&mut self, name: Option<String>, owner: Option<String>) -> NodeIndex {
        self.add(NamedWitTypeNode {
            name,
            owner,
            type_: WitTypeNode::RecordType(Vec::new()),
        })
    }

    pub(crate) fn add_variant(&mut self, name: Option<String>, owner: Option<String>) -> NodeIndex {
        self.add(NamedWitTypeNode {
            name,
            owner,
            type_: WitTypeNode::VariantType(Vec::new()),
        })
    }

    pub(crate) fn add_enum(
        &mut self,
        name: Option<String>,
        owner: Option<String>,
        values: Vec<String>,
    ) -> NodeIndex {
        self.add(NamedWitTypeNode {
            name,
            owner,
            type_: WitTypeNode::EnumType(values),
        })
    }

    pub(crate) fn add_flags(
        &mut self,
        name: Option<String>,
        owner: Option<String>,
        values: Vec<String>,
    ) -> NodeIndex {
        self.add(NamedWitTypeNode {
            name,
            owner,
            type_: WitTypeNode::FlagsType(values),
        })
    }

    pub(crate) fn add_tuple(&mut self, name: Option<String>, owner: Option<String>) -> NodeIndex {
        self.add(NamedWitTypeNode {
            name,
            owner,
            type_: WitTypeNode::TupleType(Vec::new()),
        })
    }

    pub(crate) fn add_list(&mut self, name: Option<String>, owner: Option<String>) -> NodeIndex {
        self.add(NamedWitTypeNode {
            name,
            owner,
            type_: WitTypeNode::ListType(-1 as NodeIndex),
        })
    }

    pub(crate) fn add_option(&mut self, name: Option<String>, owner: Option<String>) -> NodeIndex {
        self.add(NamedWitTypeNode {
            name,
            owner,
            type_: WitTypeNode::OptionType(-1 as NodeIndex),
        })
    }

    pub(crate) fn add_result(&mut self, name: Option<String>, owner: Option<String>) -> NodeIndex {
        self.add(NamedWitTypeNode {
            name,
            owner,
            type_: WitTypeNode::ResultType((None, None)),
        })
    }

    pub(crate) fn add_handle(
        &mut self,
        name: Option<String>,
        owner: Option<String>,
        resource_id: ResourceId,
        resource_mode: ResourceMode,
    ) -> NodeIndex {
        self.add(NamedWitTypeNode {
            name,
            owner,
            type_: WitTypeNode::HandleType((resource_id, resource_mode)),
        })
    }

    pub(crate) fn finish_container(&mut self, container_idx: NodeIndex, item_idx: NodeIndex) {
        match &mut self.nodes[container_idx as usize] {
            NamedWitTypeNode {
                type_: WitTypeNode::ListType(ref mut idx),
                ..
            } => {
                *idx = item_idx;
            }
            NamedWitTypeNode {
                type_: WitTypeNode::OptionType(ref mut idx),
                ..
            } => {
                *idx = item_idx;
            }
            _ => {
                panic!("finish_container called on node that is neither a list or an option");
            }
        }
    }

    pub(crate) fn finish_record(
        &mut self,
        record_idx: NodeIndex,
        fields: Vec<(String, NodeIndex)>,
    ) {
        match &mut self.nodes[record_idx as usize] {
            NamedWitTypeNode {
                type_: WitTypeNode::RecordType(ref mut field_list),
                ..
            } => {
                *field_list = fields;
            }
            _ => {
                panic!("finish_record called on node that is not a record");
            }
        }
    }

    pub(crate) fn finish_tuple(&mut self, tuple_idx: NodeIndex, fields: Vec<NodeIndex>) {
        match &mut self.nodes[tuple_idx as usize] {
            NamedWitTypeNode {
                type_: WitTypeNode::TupleType(ref mut field_list),
                ..
            } => {
                *field_list = fields;
            }
            _ => {
                panic!("finish_tuple called on node that is not a tuple");
            }
        }
    }

    pub(crate) fn finish_variant(
        &mut self,
        variant_idx: NodeIndex,
        cases: Vec<(String, Option<NodeIndex>)>,
    ) {
        match &mut self.nodes[variant_idx as usize] {
            NamedWitTypeNode {
                type_: WitTypeNode::VariantType(ref mut case_list),
                ..
            } => {
                *case_list = cases;
            }
            _ => {
                panic!("finish_variant called on node that is not a variant");
            }
        }
    }

    pub(crate) fn finish_result(
        &mut self,
        result_idx: NodeIndex,
        ok: Option<NodeIndex>,
        err: Option<NodeIndex>,
    ) {
        match &mut self.nodes[result_idx as usize] {
            NamedWitTypeNode {
                type_: WitTypeNode::ResultType(ref mut result),
                ..
            } => {
                *result = (ok, err);
            }
            _ => {
                panic!("finish_result called on node that is not a result");
            }
        }
    }

    pub(crate) fn build(self) -> WitType {
        WitType { nodes: self.nodes }
    }
}

impl TypeNodeBuilder for WitTypeBuilder {
    type Result = WitType;

    fn parent_builder(&mut self) -> &mut WitTypeBuilder {
        self
    }

    fn u8(mut self) -> Self::Result {
        let _ = self.add_u8();
        self.build()
    }

    fn u16(mut self) -> Self::Result {
        let _ = self.add_u16();
        self.build()
    }

    fn u32(mut self) -> Self::Result {
        let _ = self.add_u32();
        self.build()
    }

    fn u64(mut self) -> Self::Result {
        let _ = self.add_u64();
        self.build()
    }

    fn s8(mut self) -> Self::Result {
        let _ = self.add_s8();
        self.build()
    }

    fn s16(mut self) -> Self::Result {
        let _ = self.add_s16();
        self.build()
    }

    fn s32(mut self) -> Self::Result {
        let _ = self.add_s32();
        self.build()
    }

    fn s64(mut self) -> Self::Result {
        let _ = self.add_s64();
        self.build()
    }

    fn f32(mut self) -> Self::Result {
        let _ = self.add_f32();
        self.build()
    }

    fn f64(mut self) -> Self::Result {
        let _ = self.add_f64();
        self.build()
    }

    fn string(mut self) -> Self::Result {
        let _ = self.add_string();
        self.build()
    }

    fn bool(mut self) -> Self::Result {
        let _ = self.add_bool();
        self.build()
    }

    fn char(mut self) -> Self::Result {
        let _ = self.add_char();
        self.build()
    }

    fn option(
        mut self,
        name: Option<String>,
        owner: Option<String>,
    ) -> WitTypeContainerBuilder<WitTypeBuilder> {
        let option_idx = self.add_option(name, owner);
        WitTypeContainerBuilder {
            builder: self,
            target_idx: option_idx,
        }
    }

    fn list(
        mut self,
        name: Option<String>,
        owner: Option<String>,
    ) -> WitTypeContainerBuilder<WitTypeBuilder> {
        let list_idx = self.add_list(name, owner);
        WitTypeContainerBuilder {
            builder: self,
            target_idx: list_idx,
        }
    }

    fn r#enum(
        mut self,
        name: Option<String>,
        owner: Option<String>,
        values: &[&str],
    ) -> Self::Result {
        let _ = self.add_enum(name, owner, values.iter().map(|s| s.to_string()).collect());
        self.build()
    }

    fn flags(
        mut self,
        name: Option<String>,
        owner: Option<String>,
        values: &[&str],
    ) -> Self::Result {
        let _ = self.add_enum(name, owner, values.iter().map(|s| s.to_string()).collect());
        self.build()
    }

    fn record(mut self, name: Option<String>, owner: Option<String>) -> WitTypeRecordBuilder<Self> {
        let record_idx = self.add_record(name, owner);
        WitTypeRecordBuilder {
            builder: self,
            target_idx: record_idx,
            fields: Vec::new(),
        }
    }

    fn tuple(mut self, name: Option<String>, owner: Option<String>) -> WitTypeTupleBuilder<Self> {
        let tuple_idx = self.add_tuple(name, owner);
        WitTypeTupleBuilder {
            builder: self,
            target_idx: tuple_idx,
            fields: Vec::new(),
        }
    }

    fn variant(
        mut self,
        name: Option<String>,
        owner: Option<String>,
    ) -> WitTypeVariantBuilder<Self> {
        let variant_idx = self.add_variant(name, owner);
        WitTypeVariantBuilder {
            builder: self,
            target_idx: variant_idx,
            cases: Vec::new(),
        }
    }

    fn result(mut self, name: Option<String>, owner: Option<String>) -> WitTypeResultBuilder<Self> {
        let result_idx = self.add_result(name, owner);
        WitTypeResultBuilder {
            builder: self,
            target_idx: result_idx,
            ok: None,
            err: None,
        }
    }

    fn handle(
        mut self,
        name: Option<String>,
        owner: Option<String>,
        resource_id: ResourceId,
        resource_mode: ResourceMode,
    ) -> Self::Result {
        let _ = self.add_handle(name, owner, resource_id, resource_mode);
        self.build()
    }

    fn finish(self) -> Self::Result {
        self.build()
    }
}

pub struct WitTypeContainerBuilder<ParentBuilder: TypeNodeBuilder> {
    builder: ParentBuilder,
    target_idx: NodeIndex,
}

impl<ParentBuilder: TypeNodeBuilder> TypeNodeBuilder for WitTypeContainerBuilder<ParentBuilder> {
    type Result = ParentBuilder;

    fn parent_builder(&mut self) -> &mut WitTypeBuilder {
        self.builder.parent_builder()
    }

    fn u8(mut self) -> Self::Result {
        let child_index = self.parent_builder().add_u8();
        self.builder
            .parent_builder()
            .finish_container(self.target_idx, child_index);
        self.builder
    }

    fn u16(mut self) -> Self::Result {
        let child_index = self.parent_builder().add_u16();
        self.builder
            .parent_builder()
            .finish_container(self.target_idx, child_index);
        self.builder
    }

    fn u32(mut self) -> Self::Result {
        let child_index = self.parent_builder().add_u32();
        self.builder
            .parent_builder()
            .finish_container(self.target_idx, child_index);
        self.builder
    }

    fn u64(mut self) -> Self::Result {
        let child_index = self.parent_builder().add_u64();
        self.builder
            .parent_builder()
            .finish_container(self.target_idx, child_index);
        self.builder
    }

    fn s8(mut self) -> Self::Result {
        let child_index = self.parent_builder().add_s8();
        self.builder
            .parent_builder()
            .finish_container(self.target_idx, child_index);
        self.builder
    }

    fn s16(mut self) -> Self::Result {
        let child_index = self.parent_builder().add_s16();
        self.builder
            .parent_builder()
            .finish_container(self.target_idx, child_index);
        self.builder
    }

    fn s32(mut self) -> Self::Result {
        let child_index = self.parent_builder().add_s32();
        self.builder
            .parent_builder()
            .finish_container(self.target_idx, child_index);
        self.builder
    }

    fn s64(mut self) -> Self::Result {
        let child_index = self.parent_builder().add_s64();
        self.builder
            .parent_builder()
            .finish_container(self.target_idx, child_index);
        self.builder
    }

    fn f32(mut self) -> Self::Result {
        let child_index = self.parent_builder().add_f32();
        self.builder
            .parent_builder()
            .finish_container(self.target_idx, child_index);
        self.builder
    }

    fn f64(mut self) -> Self::Result {
        let child_index = self.parent_builder().add_f64();
        self.builder
            .parent_builder()
            .finish_container(self.target_idx, child_index);
        self.builder
    }

    fn string(mut self) -> Self::Result {
        let child_index = self.parent_builder().add_string();
        self.builder
            .parent_builder()
            .finish_container(self.target_idx, child_index);
        self.builder
    }

    fn bool(mut self) -> Self::Result {
        let child_index = self.parent_builder().add_bool();
        self.builder
            .parent_builder()
            .finish_container(self.target_idx, child_index);
        self.builder
    }

    fn char(mut self) -> Self::Result {
        let child_index = self.parent_builder().add_char();
        self.builder
            .parent_builder()
            .finish_container(self.target_idx, child_index);
        self.builder
    }

    fn option(
        mut self,
        name: Option<String>,
        owner: Option<String>,
    ) -> WitTypeContainerBuilder<Self> {
        let option_idx = self.parent_builder().add_option(name, owner);
        self.builder
            .parent_builder()
            .finish_container(self.target_idx, option_idx);
        WitTypeContainerBuilder {
            builder: self,
            target_idx: option_idx,
        }
    }

    fn list(
        mut self,
        name: Option<String>,
        owner: Option<String>,
    ) -> WitTypeContainerBuilder<Self> {
        let list_idx = self.parent_builder().add_list(name, owner);
        self.builder
            .parent_builder()
            .finish_container(self.target_idx, list_idx);
        WitTypeContainerBuilder {
            builder: self,
            target_idx: list_idx,
        }
    }

    fn r#enum(
        mut self,
        name: Option<String>,
        owner: Option<String>,
        values: &[&str],
    ) -> Self::Result {
        let child_index = self.parent_builder().add_enum(
            name,
            owner,
            values.iter().map(|s| s.to_string()).collect(),
        );
        self.builder
            .parent_builder()
            .finish_container(self.target_idx, child_index);
        self.builder
    }

    fn flags(
        mut self,
        name: Option<String>,
        owner: Option<String>,
        values: &[&str],
    ) -> Self::Result {
        let child_index = self.parent_builder().add_flags(
            name,
            owner,
            values.iter().map(|s| s.to_string()).collect(),
        );
        self.builder
            .parent_builder()
            .finish_container(self.target_idx, child_index);
        self.builder
    }

    fn record(mut self, name: Option<String>, owner: Option<String>) -> WitTypeRecordBuilder<Self> {
        let record_idx = self.parent_builder().add_record(name, owner);
        self.builder
            .parent_builder()
            .finish_container(self.target_idx, record_idx);
        WitTypeRecordBuilder {
            builder: self,
            target_idx: record_idx,
            fields: Vec::new(),
        }
    }

    fn tuple(mut self, name: Option<String>, owner: Option<String>) -> WitTypeTupleBuilder<Self> {
        let tuple_idx = self.parent_builder().add_tuple(name, owner);
        self.builder
            .parent_builder()
            .finish_container(self.target_idx, tuple_idx);
        WitTypeTupleBuilder {
            builder: self,
            target_idx: tuple_idx,
            fields: Vec::new(),
        }
    }

    fn variant(
        mut self,
        name: Option<String>,
        owner: Option<String>,
    ) -> WitTypeVariantBuilder<Self> {
        let variant_idx = self.parent_builder().add_variant(name, owner);
        self.builder
            .parent_builder()
            .finish_container(self.target_idx, variant_idx);
        WitTypeVariantBuilder {
            builder: self,
            target_idx: variant_idx,
            cases: Vec::new(),
        }
    }

    fn result(mut self, name: Option<String>, owner: Option<String>) -> WitTypeResultBuilder<Self> {
        let result_idx = self.parent_builder().add_result(name, owner);
        self.builder
            .parent_builder()
            .finish_container(self.target_idx, result_idx);
        WitTypeResultBuilder {
            builder: self,
            target_idx: result_idx,
            ok: None,
            err: None,
        }
    }

    fn handle(
        mut self,
        name: Option<String>,
        owner: Option<String>,
        resource_id: ResourceId,
        resource_mode: ResourceMode,
    ) -> Self::Result {
        let child_index = self
            .parent_builder()
            .add_handle(name, owner, resource_id, resource_mode);
        self.builder
            .parent_builder()
            .finish_container(self.target_idx, child_index);
        self.builder
    }

    fn finish(self) -> Self::Result {
        self.builder
    }
}

pub struct WitTypeRecordBuilder<ParentBuilder: TypeNodeBuilder> {
    builder: ParentBuilder,
    target_idx: NodeIndex,
    fields: Vec<(String, NodeIndex)>,
}

impl<ParentBuilder: TypeNodeBuilder> WitTypeRecordBuilder<ParentBuilder> {
    pub fn field(self, name: &str) -> WitTypeRecordFieldBuilder<ParentBuilder> {
        WitTypeRecordFieldBuilder {
            builder: self,
            name: name.to_string(),
        }
    }

    pub fn finish(mut self) -> ParentBuilder::Result {
        self.builder
            .parent_builder()
            .finish_record(self.target_idx, self.fields);
        self.builder.finish()
    }

    fn add(&mut self, name: String, field_idx: NodeIndex) {
        self.fields.push((name, field_idx));
    }
}

pub struct WitTypeRecordFieldBuilder<ParentBuilder: TypeNodeBuilder> {
    builder: WitTypeRecordBuilder<ParentBuilder>,
    name: String,
}

impl<ParentBuilder: TypeNodeBuilder> WitTypeRecordFieldBuilder<ParentBuilder> {
    fn finish(mut self, field_idx: NodeIndex) -> WitTypeRecordBuilder<ParentBuilder> {
        self.apply(field_idx);
        self.builder
    }

    fn apply(&mut self, field_idx: NodeIndex) {
        self.builder.add(self.name.clone(), field_idx);
    }
}

impl<ParentBuilder: TypeNodeBuilder> InnerTypeNodeBuilder
    for WitTypeRecordFieldBuilder<ParentBuilder>
{
    type Result = WitTypeRecordBuilder<ParentBuilder>;

    fn parent_builder(&mut self) -> &mut WitTypeBuilder {
        self.builder.builder.parent_builder()
    }

    fn finish(self, result_index: NodeIndex) -> Self::Result {
        self.finish(result_index)
    }

    fn apply(&mut self, result_index: NodeIndex) {
        self.apply(result_index);
    }

    fn into_parent(self) -> Self::Result {
        self.builder
    }
}

pub struct WitTypeTupleBuilder<ParentBuilder: TypeNodeBuilder> {
    builder: ParentBuilder,
    target_idx: NodeIndex,
    fields: Vec<NodeIndex>,
}

impl<ParentBuilder: TypeNodeBuilder> WitTypeTupleBuilder<ParentBuilder> {
    pub fn item(self) -> WitTypeTupleItemBuilder<ParentBuilder> {
        WitTypeTupleItemBuilder { builder: self }
    }

    pub fn finish(mut self) -> ParentBuilder::Result {
        self.builder
            .parent_builder()
            .finish_tuple(self.target_idx, self.fields);
        self.builder.finish()
    }

    fn add(&mut self, field_idx: NodeIndex) {
        self.fields.push(field_idx);
    }
}

pub struct WitTypeTupleItemBuilder<ParentBuilder: TypeNodeBuilder> {
    builder: WitTypeTupleBuilder<ParentBuilder>,
}

impl<ParentBuilder: TypeNodeBuilder> WitTypeTupleItemBuilder<ParentBuilder> {
    fn finish(mut self, field_idx: NodeIndex) -> WitTypeTupleBuilder<ParentBuilder> {
        self.apply(field_idx);
        self.builder
    }

    fn apply(&mut self, field_idx: NodeIndex) {
        self.builder.add(field_idx);
    }
}

impl<ParentBuilder: TypeNodeBuilder> InnerTypeNodeBuilder
    for WitTypeTupleItemBuilder<ParentBuilder>
{
    type Result = WitTypeTupleBuilder<ParentBuilder>;

    fn parent_builder(&mut self) -> &mut WitTypeBuilder {
        self.builder.builder.parent_builder()
    }

    fn finish(self, result_index: NodeIndex) -> Self::Result {
        self.finish(result_index)
    }

    fn apply(&mut self, result_index: NodeIndex) {
        self.apply(result_index);
    }

    fn into_parent(self) -> Self::Result {
        self.builder
    }
}

pub struct WitTypeVariantBuilder<ParentBuilder: TypeNodeBuilder> {
    builder: ParentBuilder,
    target_idx: NodeIndex,
    cases: Vec<(String, Option<NodeIndex>)>,
}

impl<ParentBuilder: TypeNodeBuilder> WitTypeVariantBuilder<ParentBuilder> {
    pub fn case(self, name: &str) -> WitTypeVariantCaseBuilder<ParentBuilder> {
        WitTypeVariantCaseBuilder {
            builder: self,
            name: name.to_string(),
        }
    }

    pub fn unit_case(mut self, name: &str) -> Self {
        self.cases.push((name.to_string(), None));
        self
    }

    pub fn finish(mut self) -> ParentBuilder::Result {
        self.builder
            .parent_builder()
            .finish_variant(self.target_idx, self.cases);
        self.builder.finish()
    }

    fn add(&mut self, name: String, case_idx: NodeIndex) {
        self.cases.push((name, Some(case_idx)));
    }
}

pub struct WitTypeVariantCaseBuilder<ParentBuilder: TypeNodeBuilder> {
    builder: WitTypeVariantBuilder<ParentBuilder>,
    name: String,
}

impl<ParentBuilder: TypeNodeBuilder> WitTypeVariantCaseBuilder<ParentBuilder> {
    fn finish(mut self, field_idx: NodeIndex) -> WitTypeVariantBuilder<ParentBuilder> {
        self.apply(field_idx);
        self.builder
    }

    fn apply(&mut self, field_idx: NodeIndex) {
        self.builder.add(self.name.clone(), field_idx);
    }
}

impl<ParentBuilder: TypeNodeBuilder> InnerTypeNodeBuilder
    for WitTypeVariantCaseBuilder<ParentBuilder>
{
    type Result = WitTypeVariantBuilder<ParentBuilder>;

    fn parent_builder(&mut self) -> &mut WitTypeBuilder {
        self.builder.builder.parent_builder()
    }

    fn finish(self, result_index: NodeIndex) -> Self::Result {
        self.finish(result_index)
    }

    fn apply(&mut self, result_index: NodeIndex) {
        self.apply(result_index);
    }

    fn into_parent(self) -> Self::Result {
        self.builder
    }
}

pub struct WitTypeResultBuilder<ParentBuilder: TypeNodeBuilder> {
    builder: ParentBuilder,
    target_idx: NodeIndex,
    ok: Option<NodeIndex>,
    err: Option<NodeIndex>,
}

impl<ParentBuilder: TypeNodeBuilder> WitTypeResultBuilder<ParentBuilder> {
    pub fn ok(self) -> WitTypeResultCaseBuilder<ParentBuilder> {
        WitTypeResultCaseBuilder {
            builder: self,
            target: Ok(()),
        }
    }

    pub fn ok_unit(mut self) -> Self {
        self.ok = None;
        self
    }

    pub fn err(self) -> WitTypeResultCaseBuilder<ParentBuilder> {
        WitTypeResultCaseBuilder {
            builder: self,
            target: Err(()),
        }
    }

    pub fn err_unit(mut self) -> Self {
        self.err = None;
        self
    }

    pub fn finish(mut self) -> ParentBuilder::Result {
        self.builder
            .parent_builder()
            .finish_result(self.target_idx, self.ok, self.err);
        self.builder.finish()
    }

    fn set(&mut self, case_idx: Result<NodeIndex, NodeIndex>) {
        match case_idx {
            Ok(idx) => self.ok = Some(idx),
            Err(idx) => self.err = Some(idx),
        }
    }
}

pub struct WitTypeResultCaseBuilder<ParentBuilder: TypeNodeBuilder> {
    builder: WitTypeResultBuilder<ParentBuilder>,
    target: Result<(), ()>,
}

impl<ParentBuilder: TypeNodeBuilder> WitTypeResultCaseBuilder<ParentBuilder> {
    fn finish(mut self, case_idx: NodeIndex) -> WitTypeResultBuilder<ParentBuilder> {
        self.apply(case_idx);
        self.builder
    }

    fn apply(&mut self, case_idx: NodeIndex) {
        self.builder
            .set(self.target.map(|_| case_idx).map_err(|_| case_idx));
    }
}

impl<ParentBuilder: TypeNodeBuilder> InnerTypeNodeBuilder
    for WitTypeResultCaseBuilder<ParentBuilder>
{
    type Result = WitTypeResultBuilder<ParentBuilder>;

    fn parent_builder(&mut self) -> &mut WitTypeBuilder {
        self.builder.builder.parent_builder()
    }

    fn finish(self, result_index: NodeIndex) -> Self::Result {
        self.finish(result_index)
    }

    fn apply(&mut self, result_index: NodeIndex) {
        self.apply(result_index);
    }

    fn into_parent(self) -> Self::Result {
        self.builder
    }
}

pub trait InnerTypeNodeBuilder {
    type Result;
    fn parent_builder(&mut self) -> &mut WitTypeBuilder;
    fn finish(self, result_index: NodeIndex) -> Self::Result;
    fn apply(&mut self, result_index: NodeIndex);
    fn into_parent(self) -> Self::Result;
}

impl<B: InnerTypeNodeBuilder> TypeNodeBuilder for B {
    type Result = B::Result;

    fn parent_builder(&mut self) -> &mut WitTypeBuilder {
        self.parent_builder()
    }

    fn u8(mut self) -> Self::Result {
        let child_index = self.parent_builder().add_u8();
        self.finish(child_index)
    }

    fn u16(mut self) -> Self::Result {
        let child_index = self.parent_builder().add_u16();
        self.finish(child_index)
    }

    fn u32(mut self) -> Self::Result {
        let child_index = self.parent_builder().add_u32();
        self.finish(child_index)
    }

    fn u64(mut self) -> Self::Result {
        let child_index = self.parent_builder().add_u64();
        self.finish(child_index)
    }

    fn s8(mut self) -> Self::Result {
        let child_index = self.parent_builder().add_s8();
        self.finish(child_index)
    }

    fn s16(mut self) -> Self::Result {
        let child_index = self.parent_builder().add_s16();
        self.finish(child_index)
    }

    fn s32(mut self) -> Self::Result {
        let child_index = self.parent_builder().add_s32();
        self.finish(child_index)
    }

    fn s64(mut self) -> Self::Result {
        let child_index = self.parent_builder().add_s64();
        self.finish(child_index)
    }

    fn f32(mut self) -> Self::Result {
        let child_index = self.parent_builder().add_f32();
        self.finish(child_index)
    }

    fn f64(mut self) -> Self::Result {
        let child_index = self.parent_builder().add_f64();
        self.finish(child_index)
    }

    fn string(mut self) -> Self::Result {
        let child_index = self.parent_builder().add_string();
        self.finish(child_index)
    }

    fn bool(mut self) -> Self::Result {
        let child_index = self.parent_builder().add_bool();
        self.finish(child_index)
    }

    fn char(mut self) -> Self::Result {
        let child_index = self.parent_builder().add_char();
        self.finish(child_index)
    }

    fn option(
        mut self,
        name: Option<String>,
        owner: Option<String>,
    ) -> WitTypeContainerBuilder<Self> {
        let option_idx = self.parent_builder().add_option(name, owner);
        self.apply(option_idx);
        WitTypeContainerBuilder {
            builder: self,
            target_idx: option_idx,
        }
    }

    fn list(
        mut self,
        name: Option<String>,
        owner: Option<String>,
    ) -> WitTypeContainerBuilder<Self> {
        let list_idx = self.parent_builder().add_list(name, owner);
        self.apply(list_idx);
        WitTypeContainerBuilder {
            builder: self,
            target_idx: list_idx,
        }
    }

    fn r#enum(
        mut self,
        name: Option<String>,
        owner: Option<String>,
        values: &[&str],
    ) -> Self::Result {
        let child_index = self.parent_builder().add_enum(
            name,
            owner,
            values.iter().map(|s| s.to_string()).collect(),
        );
        self.finish(child_index)
    }

    fn flags(
        mut self,
        name: Option<String>,
        owner: Option<String>,
        values: &[&str],
    ) -> Self::Result {
        let child_index = self.parent_builder().add_flags(
            name,
            owner,
            values.iter().map(|s| s.to_string()).collect(),
        );
        self.finish(child_index)
    }

    fn record(mut self, name: Option<String>, owner: Option<String>) -> WitTypeRecordBuilder<Self> {
        let record_idx = self.parent_builder().add_record(name, owner);
        self.apply(record_idx);
        WitTypeRecordBuilder {
            builder: self,
            target_idx: record_idx,
            fields: Vec::new(),
        }
    }

    fn tuple(mut self, name: Option<String>, owner: Option<String>) -> WitTypeTupleBuilder<Self> {
        let tuple_idx = self.parent_builder().add_tuple(name, owner);
        self.apply(tuple_idx);
        WitTypeTupleBuilder {
            builder: self,
            target_idx: tuple_idx,
            fields: Vec::new(),
        }
    }

    fn variant(
        mut self,
        name: Option<String>,
        owner: Option<String>,
    ) -> WitTypeVariantBuilder<Self> {
        let variant_idx = self.parent_builder().add_variant(name, owner);
        self.apply(variant_idx);
        WitTypeVariantBuilder {
            builder: self,
            target_idx: variant_idx,
            cases: Vec::new(),
        }
    }

    fn result(mut self, name: Option<String>, owner: Option<String>) -> WitTypeResultBuilder<Self> {
        let result_idx = self.parent_builder().add_result(name, owner);
        self.apply(result_idx);
        WitTypeResultBuilder {
            builder: self,
            target_idx: result_idx,
            ok: None,
            err: None,
        }
    }

    fn handle(
        mut self,
        name: Option<String>,
        owner: Option<String>,
        resource_id: ResourceId,
        resource_mode: ResourceMode,
    ) -> Self::Result {
        let child_index = self
            .parent_builder()
            .add_handle(name, owner, resource_id, resource_mode);
        self.finish(child_index)
    }

    fn finish(self) -> Self::Result {
        self.into_parent()
    }
}
