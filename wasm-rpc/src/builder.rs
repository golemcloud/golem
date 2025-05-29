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

use crate::{NodeIndex, Uri, WitNode, WitValue};

pub trait WitValueBuilderExtensions {
    fn builder() -> WitValueBuilder;
}

impl WitValueBuilderExtensions for WitValue {
    fn builder() -> WitValueBuilder {
        WitValueBuilder::new()
    }
}

pub trait NodeBuilder: Sized {
    type Result;

    fn parent_builder(&mut self) -> &mut WitValueBuilder;

    fn u8(self, value: u8) -> Self::Result;
    fn u16(self, value: u16) -> Self::Result;
    fn u32(self, value: u32) -> Self::Result;
    fn u64(self, value: u64) -> Self::Result;

    fn s8(self, value: i8) -> Self::Result;
    fn s16(self, value: i16) -> Self::Result;
    fn s32(self, value: i32) -> Self::Result;
    fn s64(self, value: i64) -> Self::Result;

    fn f32(self, value: f32) -> Self::Result;
    fn f64(self, value: f64) -> Self::Result;

    fn char(self, value: char) -> Self::Result;
    fn bool(self, value: bool) -> Self::Result;
    fn string(self, value: &str) -> Self::Result;
    fn enum_value(self, value: u32) -> Self::Result;
    fn flags(self, values: Vec<bool>) -> Self::Result;

    fn record(self) -> WitValueChildItemsBuilder<Self>;
    fn variant(self, case_idx: u32) -> WitValueChildBuilder<Self>;
    fn variant_unit(self, case_idx: u32) -> Self::Result;

    /// An alternative to `variant` and `variant_unit`, easier to use in generated code
    fn variant_fn(
        self,
        case_idx: u32,
        is_unit: bool,
        f: impl FnOnce(WitValueChildBuilder<Self>) -> Self,
    ) -> Self::Result {
        if is_unit {
            self.variant_unit(case_idx)
        } else {
            f(self.variant(case_idx)).finish()
        }
    }

    fn tuple(self) -> WitValueChildItemsBuilder<Self>;
    fn list(self) -> WitValueChildItemsBuilder<Self>;

    fn list_fn<T>(
        self,
        items: &[T],
        f: impl Fn(&T, WitValueItemBuilder<Self>) -> WitValueChildItemsBuilder<Self>,
    ) -> Self::Result {
        let mut builder = self.list();
        for item in items {
            builder = f(item, builder.item());
        }
        builder.finish()
    }

    fn option_some(self) -> WitValueChildBuilder<Self>;
    fn option_none(self) -> Self::Result;

    /// An alternative to `option_some` and `option_none`, easier to use in generated code
    fn option_fn(
        self,
        is_some: bool,
        f: impl FnOnce(WitValueChildBuilder<Self>) -> Self,
    ) -> Self::Result {
        if is_some {
            f(self.option_some()).finish()
        } else {
            self.option_none()
        }
    }

    fn result_ok(self) -> WitValueChildBuilder<Self>;
    fn result_ok_unit(self) -> Self::Result;
    fn result_err(self) -> WitValueChildBuilder<Self>;
    fn result_err_unit(self) -> Self::Result;

    /// An alternative to `result_ok`, `result_ok_unit`, `result_err` and `result_err_unit`, easier to use in generated code
    fn result_fn(
        self,
        is_ok: bool,
        has_ok: bool,
        has_err: bool,
        f: impl FnOnce(WitValueChildBuilder<Self>) -> Self,
    ) -> Self::Result {
        if is_ok {
            if has_ok {
                f(self.result_ok()).finish()
            } else {
                self.result_ok_unit()
            }
        } else if has_err {
            f(self.result_err()).finish()
        } else {
            self.result_err_unit()
        }
    }

    fn handle(self, uri: Uri, handle_value: u64) -> Self::Result;

    fn finish(self) -> Self::Result;
}

pub struct WitValueBuilder {
    nodes: Vec<WitNode>,
}

impl WitValueBuilder {
    pub(crate) fn new() -> Self {
        WitValueBuilder { nodes: Vec::new() }
    }

    fn add(&mut self, node: WitNode) -> NodeIndex {
        self.nodes.push(node);
        self.nodes.len() as NodeIndex - 1
    }

    pub(crate) fn add_u8(&mut self, value: u8) -> NodeIndex {
        self.add(WitNode::PrimU8(value))
    }

    pub(crate) fn add_u16(&mut self, value: u16) -> NodeIndex {
        self.add(WitNode::PrimU16(value))
    }

    pub(crate) fn add_u32(&mut self, value: u32) -> NodeIndex {
        self.add(WitNode::PrimU32(value))
    }

    pub(crate) fn add_u64(&mut self, value: u64) -> NodeIndex {
        self.add(WitNode::PrimU64(value))
    }

    pub(crate) fn add_s8(&mut self, value: i8) -> NodeIndex {
        self.add(WitNode::PrimS8(value))
    }

    pub(crate) fn add_s16(&mut self, value: i16) -> NodeIndex {
        self.add(WitNode::PrimS16(value))
    }

    pub(crate) fn add_s32(&mut self, value: i32) -> NodeIndex {
        self.add(WitNode::PrimS32(value))
    }

    pub(crate) fn add_s64(&mut self, value: i64) -> NodeIndex {
        self.add(WitNode::PrimS64(value))
    }

    pub(crate) fn add_f32(&mut self, value: f32) -> NodeIndex {
        self.add(WitNode::PrimFloat32(value))
    }

    pub(crate) fn add_f64(&mut self, value: f64) -> NodeIndex {
        self.add(WitNode::PrimFloat64(value))
    }

    pub(crate) fn add_char(&mut self, value: char) -> NodeIndex {
        self.add(WitNode::PrimChar(value))
    }

    pub(crate) fn add_bool(&mut self, value: bool) -> NodeIndex {
        self.add(WitNode::PrimBool(value))
    }

    pub(crate) fn add_string(&mut self, value: &str) -> NodeIndex {
        self.add(WitNode::PrimString(value.to_string()))
    }

    pub(crate) fn add_record(&mut self) -> NodeIndex {
        self.add(WitNode::RecordValue(Vec::new()))
    }

    pub(crate) fn add_variant(&mut self, idx: u32, target_idx: NodeIndex) -> NodeIndex {
        self.add(WitNode::VariantValue((idx, Some(target_idx))))
    }

    pub(crate) fn add_variant_unit(&mut self, idx: u32) -> NodeIndex {
        self.add(WitNode::VariantValue((idx, None)))
    }

    pub(crate) fn add_enum_value(&mut self, value: u32) -> NodeIndex {
        self.add(WitNode::EnumValue(value))
    }

    pub(crate) fn add_flags(&mut self, values: Vec<bool>) -> NodeIndex {
        self.add(WitNode::FlagsValue(values))
    }

    pub(crate) fn add_tuple(&mut self) -> NodeIndex {
        self.add(WitNode::TupleValue(Vec::new()))
    }

    pub(crate) fn add_list(&mut self) -> NodeIndex {
        self.add(WitNode::ListValue(Vec::new()))
    }

    pub(crate) fn add_option_none(&mut self) -> NodeIndex {
        self.add(WitNode::OptionValue(None))
    }

    pub(crate) fn add_option_some(&mut self) -> NodeIndex {
        self.add(WitNode::OptionValue(Some(-1)))
    }

    pub(crate) fn add_result_ok(&mut self) -> NodeIndex {
        self.add(WitNode::ResultValue(Ok(Some(-1))))
    }

    pub(crate) fn add_result_ok_unit(&mut self) -> NodeIndex {
        self.add(WitNode::ResultValue(Ok(None)))
    }

    pub(crate) fn add_result_err(&mut self) -> NodeIndex {
        self.add(WitNode::ResultValue(Err(Some(-1))))
    }

    pub(crate) fn add_result_err_unit(&mut self) -> NodeIndex {
        self.add(WitNode::ResultValue(Err(None)))
    }

    pub(crate) fn add_handle(&mut self, uri: Uri, handle_value: u64) -> NodeIndex {
        self.add(WitNode::Handle((uri, handle_value)))
    }

    pub(crate) fn finish_child(&mut self, child: NodeIndex, target_idx: NodeIndex) {
        match &mut self.nodes[target_idx as usize] {
            WitNode::OptionValue(ref mut result_item) => match result_item {
                Some(idx) => *idx = child,
                None => panic!("finish_child called on None option"),
            },
            WitNode::ResultValue(ref mut result_item) => match result_item {
                Ok(Some(idx)) => *idx = child,
                Ok(None) => panic!("finish_child called on Ok(None) result"),
                Err(Some(idx)) => *idx = child,
                Err(None) => panic!("finish_child called on Err(None) result"),
            },
            WitNode::VariantValue((_, ref mut result_item)) => match result_item {
                Some(idx) => *idx = child,
                None => panic!("finish_child called on variant with no inner value"),
            },
            _ => {
                panic!(
                    "finish_child called on a node that is neither an option, result or variant"
                );
            }
        }
    }

    pub(crate) fn finish_seq(&mut self, items: Vec<NodeIndex>, target_idx: NodeIndex) {
        match &mut self.nodes[target_idx as usize] {
            WitNode::RecordValue(ref mut result_items) => {
                *result_items = items;
            }
            WitNode::TupleValue(ref mut result_items) => {
                *result_items = items;
            }
            WitNode::ListValue(ref mut result_items) => {
                *result_items = items;
            }
            _ => {
                panic!("finish_seq called on a node that is neither a record, list, or tuple");
            }
        }
    }

    pub(crate) fn build(self) -> WitValue {
        WitValue { nodes: self.nodes }
    }
}

impl NodeBuilder for WitValueBuilder {
    type Result = WitValue;

    fn parent_builder(&mut self) -> &mut WitValueBuilder {
        self
    }

    fn u8(mut self, value: u8) -> Self::Result {
        let _ = self.add_u8(value);
        self.build()
    }

    fn u16(mut self, value: u16) -> Self::Result {
        let _ = self.add_u16(value);
        self.build()
    }

    fn u32(mut self, value: u32) -> Self::Result {
        let _ = self.add_u32(value);
        self.build()
    }

    fn u64(mut self, value: u64) -> Self::Result {
        let _ = self.add_u64(value);
        self.build()
    }

    fn s8(mut self, value: i8) -> Self::Result {
        let _ = self.add_s8(value);
        self.build()
    }

    fn s16(mut self, value: i16) -> Self::Result {
        let _ = self.add_s16(value);
        self.build()
    }

    fn s32(mut self, value: i32) -> Self::Result {
        let _ = self.add_s32(value);
        self.build()
    }

    fn s64(mut self, value: i64) -> Self::Result {
        let _ = self.add_s64(value);
        self.build()
    }

    fn f32(mut self, value: f32) -> Self::Result {
        let _ = self.add_f32(value);
        self.build()
    }

    fn f64(mut self, value: f64) -> Self::Result {
        let _ = self.add_f64(value);
        self.build()
    }

    fn char(mut self, value: char) -> Self::Result {
        let _ = self.add_char(value);
        self.build()
    }

    fn bool(mut self, value: bool) -> Self::Result {
        let _ = self.add_bool(value);
        self.build()
    }

    fn string(mut self, value: &str) -> Self::Result {
        let _ = self.add_string(value);
        self.build()
    }

    fn enum_value(mut self, value: u32) -> Self::Result {
        let _ = self.add_enum_value(value);
        self.build()
    }

    fn flags(mut self, values: Vec<bool>) -> Self::Result {
        let _ = self.add_flags(values);
        self.build()
    }

    fn record(mut self) -> WitValueChildItemsBuilder<WitValueBuilder> {
        let idx = self.add_record();
        WitValueChildItemsBuilder::new(self, idx)
    }

    fn variant(mut self, case_idx: u32) -> WitValueChildBuilder<WitValueBuilder> {
        let variant_idx = self.add_variant(case_idx, -1);
        WitValueChildBuilder {
            builder: self,
            target_idx: variant_idx,
        }
    }

    fn variant_unit(mut self, case_idx: u32) -> Self::Result {
        let _ = self.add_variant_unit(case_idx);
        self.build()
    }

    fn tuple(mut self) -> WitValueChildItemsBuilder<WitValueBuilder> {
        let tuple_idx = self.add_tuple();
        WitValueChildItemsBuilder::new(self, tuple_idx)
    }

    fn list(mut self) -> WitValueChildItemsBuilder<WitValueBuilder> {
        let tuple_idx = self.add_list();
        WitValueChildItemsBuilder::new(self, tuple_idx)
    }

    fn option_some(mut self) -> WitValueChildBuilder<Self> {
        let option_idx = self.add_option_some();
        WitValueChildBuilder {
            builder: self,
            target_idx: option_idx,
        }
    }

    fn option_none(mut self) -> Self::Result {
        let _ = self.add_option_none();
        self.build()
    }

    fn result_ok(mut self) -> WitValueChildBuilder<Self> {
        let result_idx = self.add_result_ok();
        WitValueChildBuilder {
            builder: self,
            target_idx: result_idx,
        }
    }

    fn result_ok_unit(mut self) -> Self::Result {
        let _ = self.add_result_ok_unit();
        self.build()
    }

    fn result_err(mut self) -> WitValueChildBuilder<Self> {
        let result_idx = self.add_result_err();
        WitValueChildBuilder {
            builder: self,
            target_idx: result_idx,
        }
    }

    fn result_err_unit(mut self) -> Self::Result {
        let _ = self.add_result_err_unit();
        self.build()
    }

    fn handle(mut self, uri: Uri, handle_value: u64) -> Self::Result {
        let _ = self.add_handle(uri, handle_value);
        self.build()
    }

    fn finish(self) -> Self::Result {
        self.build()
    }
}

pub struct WitValueChildItemsBuilder<ParentBuilder: NodeBuilder> {
    builder: ParentBuilder,
    target_idx: NodeIndex,
    items: Vec<NodeIndex>,
}

impl<ParentBuilder: NodeBuilder> WitValueChildItemsBuilder<ParentBuilder> {
    fn new(builder: ParentBuilder, target_idx: NodeIndex) -> Self {
        Self {
            builder,
            target_idx,
            items: Vec::new(),
        }
    }

    fn add_item(&mut self, item_type_index: i32) {
        self.items.push(item_type_index);
    }

    pub fn item(self) -> WitValueItemBuilder<ParentBuilder> {
        WitValueItemBuilder {
            child_items_builder: self,
        }
    }

    pub fn finish(mut self) -> ParentBuilder::Result {
        self.builder
            .parent_builder()
            .finish_seq(self.items, self.target_idx);
        self.builder.finish()
    }
}

pub struct WitValueItemBuilder<ParentBuilder: NodeBuilder> {
    child_items_builder: WitValueChildItemsBuilder<ParentBuilder>,
}

impl<ParentBuilder: NodeBuilder> NodeBuilder for WitValueItemBuilder<ParentBuilder> {
    type Result = WitValueChildItemsBuilder<ParentBuilder>;

    fn parent_builder(&mut self) -> &mut WitValueBuilder {
        self.child_items_builder.builder.parent_builder()
    }

    fn u8(mut self, value: u8) -> Self::Result {
        let item_type_index = self.parent_builder().add_u8(value);
        self.child_items_builder.add_item(item_type_index);
        self.child_items_builder
    }

    fn u16(mut self, value: u16) -> Self::Result {
        let item_type_index = self.parent_builder().add_u16(value);
        self.child_items_builder.add_item(item_type_index);
        self.child_items_builder
    }

    fn u32(mut self, value: u32) -> Self::Result {
        let item_type_index = self.parent_builder().add_u32(value);
        self.child_items_builder.add_item(item_type_index);
        self.child_items_builder
    }

    fn u64(mut self, value: u64) -> Self::Result {
        let item_type_index = self.parent_builder().add_u64(value);
        self.child_items_builder.add_item(item_type_index);
        self.child_items_builder
    }

    fn s8(mut self, value: i8) -> Self::Result {
        let item_type_index = self.parent_builder().add_s8(value);
        self.child_items_builder.add_item(item_type_index);
        self.child_items_builder
    }

    fn s16(mut self, value: i16) -> Self::Result {
        let item_type_index = self.parent_builder().add_s16(value);
        self.child_items_builder.add_item(item_type_index);
        self.child_items_builder
    }

    fn s32(mut self, value: i32) -> Self::Result {
        let item_type_index = self.parent_builder().add_s32(value);
        self.child_items_builder.add_item(item_type_index);
        self.child_items_builder
    }

    fn s64(mut self, value: i64) -> Self::Result {
        let item_type_index = self.parent_builder().add_s64(value);
        self.child_items_builder.add_item(item_type_index);
        self.child_items_builder
    }

    fn f32(mut self, value: f32) -> Self::Result {
        let item_type_index = self.parent_builder().add_f32(value);
        self.child_items_builder.add_item(item_type_index);
        self.child_items_builder
    }

    fn f64(mut self, value: f64) -> Self::Result {
        let item_type_index = self.parent_builder().add_f64(value);
        self.child_items_builder.add_item(item_type_index);
        self.child_items_builder
    }

    fn char(mut self, value: char) -> Self::Result {
        let item_type_index = self.parent_builder().add_char(value);
        self.child_items_builder.add_item(item_type_index);
        self.child_items_builder
    }

    fn bool(mut self, value: bool) -> Self::Result {
        let item_type_index = self.parent_builder().add_bool(value);
        self.child_items_builder.add_item(item_type_index);
        self.child_items_builder
    }

    fn string(mut self, value: &str) -> Self::Result {
        let item_type_index = self.parent_builder().add_string(value);
        self.child_items_builder.add_item(item_type_index);
        self.child_items_builder
    }

    fn enum_value(mut self, value: u32) -> Self::Result {
        let item_type_index = self.parent_builder().add_enum_value(value);
        self.child_items_builder.add_item(item_type_index);
        self.child_items_builder
    }

    fn flags(mut self, values: Vec<bool>) -> Self::Result {
        let item_type_index = self.parent_builder().add_flags(values);
        self.child_items_builder.add_item(item_type_index);
        self.child_items_builder
    }

    fn record(mut self) -> WitValueChildItemsBuilder<WitValueItemBuilder<ParentBuilder>> {
        let target_idx = self.parent_builder().add_record();
        self.child_items_builder.add_item(target_idx);
        WitValueChildItemsBuilder::new(self, target_idx)
    }

    fn variant(
        mut self,
        case_idx: u32,
    ) -> WitValueChildBuilder<WitValueItemBuilder<ParentBuilder>> {
        let variant_idx = self.parent_builder().add_variant(case_idx, -1);
        self.child_items_builder.add_item(variant_idx);
        WitValueChildBuilder {
            builder: self,
            target_idx: variant_idx,
        }
    }

    fn variant_unit(mut self, case_idx: u32) -> Self::Result {
        let variant_idx = self.parent_builder().add_variant_unit(case_idx);
        self.child_items_builder.add_item(variant_idx);
        self.child_items_builder
    }

    fn tuple(mut self) -> WitValueChildItemsBuilder<Self> {
        let target_idx = self.parent_builder().add_tuple();
        self.child_items_builder.add_item(target_idx);
        WitValueChildItemsBuilder::new(self, target_idx)
    }

    fn list(mut self) -> WitValueChildItemsBuilder<Self> {
        let target_idx = self.parent_builder().add_list();
        self.child_items_builder.add_item(target_idx);
        WitValueChildItemsBuilder::new(self, target_idx)
    }

    fn option_some(mut self) -> WitValueChildBuilder<Self> {
        let option_idx = self.parent_builder().add_option_some();
        self.child_items_builder.add_item(option_idx);
        WitValueChildBuilder {
            builder: self,
            target_idx: option_idx,
        }
    }

    fn option_none(mut self) -> Self::Result {
        let option_idx = self.parent_builder().add_option_none();
        self.child_items_builder.add_item(option_idx);
        self.child_items_builder
    }

    fn result_ok(mut self) -> WitValueChildBuilder<Self> {
        let result_idx = self.parent_builder().add_result_ok();
        self.child_items_builder.add_item(result_idx);
        WitValueChildBuilder {
            builder: self,
            target_idx: result_idx,
        }
    }

    fn result_ok_unit(mut self) -> Self::Result {
        let result_idx = self.parent_builder().add_result_ok_unit();
        self.child_items_builder.add_item(result_idx);
        self.child_items_builder
    }

    fn result_err(mut self) -> WitValueChildBuilder<Self> {
        let result_idx = self.parent_builder().add_result_err();
        self.child_items_builder.add_item(result_idx);
        WitValueChildBuilder {
            builder: self,
            target_idx: result_idx,
        }
    }

    fn result_err_unit(mut self) -> Self::Result {
        let result_idx = self.parent_builder().add_result_err_unit();
        self.child_items_builder.add_item(result_idx);
        self.child_items_builder
    }

    fn handle(mut self, uri: Uri, handle_value: u64) -> Self::Result {
        let item_type_index = self.parent_builder().add_handle(uri, handle_value);
        self.child_items_builder.add_item(item_type_index);
        self.child_items_builder
    }

    fn finish(self) -> Self::Result {
        self.child_items_builder
    }
}

pub struct WitValueChildBuilder<ParentBuilder: NodeBuilder> {
    builder: ParentBuilder,
    target_idx: NodeIndex,
}

impl<ParentBuilder: NodeBuilder> NodeBuilder for WitValueChildBuilder<ParentBuilder> {
    type Result = ParentBuilder;

    fn parent_builder(&mut self) -> &mut WitValueBuilder {
        self.builder.parent_builder()
    }

    fn u8(mut self, value: u8) -> Self::Result {
        let child_index = self.parent_builder().add_u8(value);
        let target_idx = self.target_idx;
        self.parent_builder().finish_child(child_index, target_idx);
        self.builder
    }

    fn u16(mut self, value: u16) -> Self::Result {
        let child_index = self.parent_builder().add_u16(value);
        let target_idx = self.target_idx;
        self.parent_builder().finish_child(child_index, target_idx);
        self.builder
    }

    fn u32(mut self, value: u32) -> Self::Result {
        let child_index = self.parent_builder().add_u32(value);
        let target_idx = self.target_idx;
        self.parent_builder().finish_child(child_index, target_idx);
        self.builder
    }

    fn u64(mut self, value: u64) -> Self::Result {
        let child_index = self.parent_builder().add_u64(value);
        let target_idx = self.target_idx;
        self.parent_builder().finish_child(child_index, target_idx);
        self.builder
    }

    fn s8(mut self, value: i8) -> Self::Result {
        let child_index = self.parent_builder().add_s8(value);
        let target_idx = self.target_idx;
        self.parent_builder().finish_child(child_index, target_idx);
        self.builder
    }

    fn s16(mut self, value: i16) -> Self::Result {
        let child_index = self.parent_builder().add_s16(value);
        let target_idx = self.target_idx;
        self.parent_builder().finish_child(child_index, target_idx);
        self.builder
    }

    fn s32(mut self, value: i32) -> Self::Result {
        let child_index = self.parent_builder().add_s32(value);
        let target_idx = self.target_idx;
        self.parent_builder().finish_child(child_index, target_idx);
        self.builder
    }

    fn s64(mut self, value: i64) -> Self::Result {
        let child_index = self.parent_builder().add_s64(value);
        let target_idx = self.target_idx;
        self.parent_builder().finish_child(child_index, target_idx);
        self.builder
    }

    fn f32(mut self, value: f32) -> Self::Result {
        let child_index = self.parent_builder().add_f32(value);
        let target_idx = self.target_idx;
        self.parent_builder().finish_child(child_index, target_idx);
        self.builder
    }

    fn f64(mut self, value: f64) -> Self::Result {
        let child_index = self.parent_builder().add_f64(value);
        let target_idx = self.target_idx;
        self.parent_builder().finish_child(child_index, target_idx);
        self.builder
    }

    fn char(mut self, value: char) -> Self::Result {
        let child_index = self.parent_builder().add_char(value);
        let target_idx = self.target_idx;
        self.parent_builder().finish_child(child_index, target_idx);
        self.builder
    }

    fn bool(mut self, value: bool) -> Self::Result {
        let child_index = self.parent_builder().add_bool(value);
        let target_idx = self.target_idx;
        self.parent_builder().finish_child(child_index, target_idx);
        self.builder
    }

    fn string(mut self, value: &str) -> Self::Result {
        let child_index = self.parent_builder().add_string(value);
        let target_idx = self.target_idx;
        self.parent_builder().finish_child(child_index, target_idx);
        self.builder
    }

    fn enum_value(mut self, value: u32) -> Self::Result {
        let child_index = self.parent_builder().add_enum_value(value);
        let target_idx = self.target_idx;
        self.parent_builder().finish_child(child_index, target_idx);
        self.builder
    }

    fn flags(mut self, values: Vec<bool>) -> Self::Result {
        let child_index = self.parent_builder().add_flags(values);
        let target_idx = self.target_idx;
        self.parent_builder().finish_child(child_index, target_idx);
        self.builder
    }

    fn record(mut self) -> WitValueChildItemsBuilder<Self> {
        let child_index = self.parent_builder().add_record();
        let target_idx = self.target_idx;
        self.parent_builder().finish_child(child_index, target_idx);
        WitValueChildItemsBuilder::new(self, child_index)
    }

    fn variant(mut self, case_idx: u32) -> WitValueChildBuilder<Self> {
        let variant_idx = self.parent_builder().add_variant(case_idx, -1);
        let target_idx = self.target_idx;
        self.parent_builder().finish_child(variant_idx, target_idx);
        WitValueChildBuilder {
            builder: self,
            target_idx: variant_idx,
        }
    }

    fn variant_unit(mut self, case_idx: u32) -> Self::Result {
        let variant_idx = self.parent_builder().add_variant_unit(case_idx);
        let target_idx = self.target_idx;
        self.parent_builder().finish_child(variant_idx, target_idx);
        self.builder
    }

    fn tuple(mut self) -> WitValueChildItemsBuilder<Self> {
        let tuple_idx = self.parent_builder().add_tuple();
        let target_idx = self.target_idx;
        self.parent_builder().finish_child(tuple_idx, target_idx);
        WitValueChildItemsBuilder::new(self, tuple_idx)
    }

    fn list(mut self) -> WitValueChildItemsBuilder<Self> {
        let list_idx = self.parent_builder().add_list();
        let target_idx = self.target_idx;
        self.parent_builder().finish_child(list_idx, target_idx);
        WitValueChildItemsBuilder::new(self, list_idx)
    }

    fn option_some(mut self) -> WitValueChildBuilder<Self> {
        let option_idx = self.parent_builder().add_option_some();
        let target_idx = self.target_idx;
        self.parent_builder().finish_child(option_idx, target_idx);
        WitValueChildBuilder {
            builder: self,
            target_idx: option_idx,
        }
    }

    fn option_none(mut self) -> Self::Result {
        let option_idx = self.parent_builder().add_option_none();
        let target_idx = self.target_idx;
        self.parent_builder().finish_child(option_idx, target_idx);
        self.builder
    }

    fn result_ok(mut self) -> WitValueChildBuilder<Self> {
        let result_idx = self.parent_builder().add_result_ok();
        let target_idx = self.target_idx;
        self.parent_builder().finish_child(result_idx, target_idx);
        WitValueChildBuilder {
            builder: self,
            target_idx: result_idx,
        }
    }

    fn result_ok_unit(mut self) -> Self::Result {
        let result_idx = self.parent_builder().add_result_ok_unit();
        let target_idx = self.target_idx;
        self.parent_builder().finish_child(result_idx, target_idx);
        self.builder
    }

    fn result_err(mut self) -> WitValueChildBuilder<Self> {
        let result_idx = self.parent_builder().add_result_err();
        let target_idx = self.target_idx;
        self.parent_builder().finish_child(result_idx, target_idx);
        WitValueChildBuilder {
            builder: self,
            target_idx: result_idx,
        }
    }

    fn result_err_unit(mut self) -> Self::Result {
        let result_idx = self.parent_builder().add_result_err_unit();
        let target_idx = self.target_idx;
        self.parent_builder().finish_child(result_idx, target_idx);
        self.builder
    }

    fn handle(mut self, uri: Uri, handle_value: u64) -> Self::Result {
        let child_index = self.parent_builder().add_handle(uri, handle_value);
        let target_idx = self.target_idx;
        self.parent_builder().finish_child(child_index, target_idx);
        self.builder
    }

    fn finish(self) -> Self::Result {
        self.builder
    }
}

#[cfg(test)]
mod tests {
    use test_r::test;

    use crate::{NodeBuilder, Value, WitValue, WitValueBuilderExtensions};

    #[test]
    fn primitive() {
        let wit_value = WitValue::builder().u64(11);
        let value: Value = wit_value.into();
        assert_eq!(value, Value::U64(11));
    }

    #[test]
    fn single_record() {
        let wit_value = WitValue::builder()
            .record()
            .item()
            .u8(1)
            .item()
            .enum_value(2)
            .item()
            .flags(vec![true, false, true])
            .finish();
        let value: Value = wit_value.into();
        assert_eq!(
            value,
            Value::Record(vec![
                Value::U8(1),
                Value::Enum(2),
                Value::Flags(vec![true, false, true]),
            ])
        );
    }

    #[test]
    fn deep_record() {
        let wit_value = WitValue::builder()
            .record()
            .item()
            .list()
            .item()
            .record()
            .item()
            .s32(10)
            .item()
            .s32(-11)
            .finish()
            .item()
            .record()
            .item()
            .s32(100)
            .item()
            .s32(200)
            .finish()
            .finish()
            .finish();
        let value: Value = wit_value.into();
        assert_eq!(
            value,
            Value::Record(vec![Value::List(vec![
                Value::Record(vec![Value::S32(10), Value::S32(-11)]),
                Value::Record(vec![Value::S32(100), Value::S32(200)]),
            ]),])
        );
    }

    #[test]
    fn option() {
        let wit_value = WitValue::builder()
            .option_some()
            .option_some()
            .option_none()
            .finish()
            .finish();
        let value: Value = wit_value.into();
        assert_eq!(
            value,
            Value::Option(Some(Box::new(Value::Option(Some(Box::new(
                Value::Option(None)
            ))))))
        );
    }
}
