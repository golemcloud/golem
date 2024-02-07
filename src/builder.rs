// TODO: get rid of the Vec<Item> for single-item cases

use crate::{TypeIndex, WitNode, WitValue};

pub trait WitValueExtensions {
    fn builder() -> WitValueBuilder;
}

impl WitValueExtensions for WitValue {
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
    fn variant(self, case_idx: u32) -> WitValueItemBuilder<Self>;
    fn tuple(self) -> WitValueChildItemsBuilder<Self>;
    fn list(self) -> WitValueChildItemsBuilder<Self>;

    fn option_some(self) -> WitValueItemBuilder<Self>;
    fn option_none(self) -> Self::Result;

    fn result_ok(self) -> WitValueItemBuilder<Self>;
    fn result_err(self) -> WitValueItemBuilder<Self>;

    fn finish(self) -> Self::Result;
}

pub struct WitValueBuilder {
    nodes: Vec<WitNode>,
}

impl WitValueBuilder {
    pub(crate) fn new() -> Self {
        WitValueBuilder { nodes: Vec::new() }
    }

    fn add(&mut self, node: WitNode) -> TypeIndex {
        self.nodes.push(node);
        self.nodes.len() as TypeIndex - 1
    }

    pub(crate) fn add_u8(&mut self, value: u8) -> TypeIndex {
        self.add(WitNode::PrimU8(value))
    }

    pub(crate) fn add_u16(&mut self, value: u16) -> TypeIndex {
        self.add(WitNode::PrimU16(value))
    }

    pub(crate) fn add_u32(&mut self, value: u32) -> TypeIndex {
        self.add(WitNode::PrimU32(value))
    }

    pub(crate) fn add_u64(&mut self, value: u64) -> TypeIndex {
        self.add(WitNode::PrimU64(value))
    }

    pub(crate) fn add_s8(&mut self, value: i8) -> TypeIndex {
        self.add(WitNode::PrimS8(value))
    }

    pub(crate) fn add_s16(&mut self, value: i16) -> TypeIndex {
        self.add(WitNode::PrimS16(value))
    }

    pub(crate) fn add_s32(&mut self, value: i32) -> TypeIndex {
        self.add(WitNode::PrimS32(value))
    }

    pub(crate) fn add_s64(&mut self, value: i64) -> TypeIndex {
        self.add(WitNode::PrimS64(value))
    }

    pub(crate) fn add_f32(&mut self, value: f32) -> TypeIndex {
        self.add(WitNode::PrimFloat32(value))
    }

    pub(crate) fn add_f64(&mut self, value: f64) -> TypeIndex {
        self.add(WitNode::PrimFloat64(value))
    }

    pub(crate) fn add_char(&mut self, value: char) -> TypeIndex {
        self.add(WitNode::PrimChar(value))
    }

    pub(crate) fn add_bool(&mut self, value: bool) -> TypeIndex {
        self.add(WitNode::PrimBool(value))
    }

    pub(crate) fn add_string(&mut self, value: &str) -> TypeIndex {
        self.add(WitNode::PrimString(value.to_string()))
    }

    pub(crate) fn add_record(&mut self) -> TypeIndex {
        self.add(WitNode::RecordValue(Vec::new()))
    }

    pub(crate) fn add_variant(&mut self, idx: u32, target_idx: TypeIndex) -> TypeIndex {
        self.add(WitNode::VariantValue((idx, target_idx)))
    }

    pub(crate) fn add_enum_value(&mut self, value: u32) -> TypeIndex {
        self.add(WitNode::EnumValue(value))
    }

    pub(crate) fn add_flags(&mut self, values: Vec<bool>) -> TypeIndex {
        self.add(WitNode::FlagsValue(values))
    }

    pub(crate) fn add_tuple(&mut self) -> TypeIndex {
        self.add(WitNode::TupleValue(Vec::new()))
    }

    pub(crate) fn add_list(&mut self) -> TypeIndex {
        self.add(WitNode::ListValue(Vec::new()))
    }

    pub(crate) fn add_option(&mut self) -> TypeIndex {
        self.add(WitNode::OptionValue(None))
    }

    pub(crate) fn add_result_ok(&mut self) -> TypeIndex {
        self.add(WitNode::ResultValue(Ok(-1)))
    }

    pub(crate) fn add_result_err(&mut self) -> TypeIndex {
        self.add(WitNode::ResultValue(Err(-1)))
    }

    pub(crate) fn finish_seq(&mut self, items: Vec<TypeIndex>, target_idx: TypeIndex) {
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
            WitNode::OptionValue(ref mut result_item) => {
                *result_item = items.first().copied();
            }
            WitNode::ResultValue(ref mut result_item) => {
                match result_item {
                    Ok(idx) => {
                        *idx = items.first().copied().expect("finish_seq called with no items for result")
                    }
                    Err(idx) => {
                        *idx = items.first().copied().expect("finish_seq called with no items for result")
                    }
                }
            }
            WitNode::VariantValue((_, ref mut result_item)) => {
                *result_item = items.first().copied().expect("finish_seq called with no items for variant");
            }
            _ => {
                panic!("finish_seq called on a node that is neither a list nor a tuple");
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

    fn variant(mut self, case_idx: u32) -> WitValueItemBuilder<WitValueBuilder> {
        let variant_idx = self.add_variant(case_idx, -1);
        WitValueItemBuilder {
            child_items_builder: WitValueChildItemsBuilder::new(self, variant_idx)
        }
    }

    fn tuple(mut self) -> WitValueChildItemsBuilder<WitValueBuilder> {
        let tuple_idx = self.add_tuple();
        WitValueChildItemsBuilder::new(self, tuple_idx)
    }

    fn list(mut self) -> WitValueChildItemsBuilder<WitValueBuilder> {
        let tuple_idx = self.add_list();
        WitValueChildItemsBuilder::new(self, tuple_idx)
    }

    fn option_some(mut self) -> WitValueItemBuilder<Self> {
        let option_idx = self.add_option();
        WitValueItemBuilder {
            child_items_builder: WitValueChildItemsBuilder::new(self, option_idx)
        }
    }

    fn option_none(mut self) -> Self::Result {
        let _ = self.add_option();
        self.build()
    }

    fn result_ok(mut self) -> WitValueItemBuilder<Self> {
        let result_idx = self.add_result_ok();
        WitValueItemBuilder {
            child_items_builder: WitValueChildItemsBuilder::new(self, result_idx)
        }
    }

    fn result_err(mut self) -> WitValueItemBuilder<Self> {
        let result_idx = self.add_result_err();
        WitValueItemBuilder {
            child_items_builder: WitValueChildItemsBuilder::new(self, result_idx)
        }
    }

    fn finish(self) -> Self::Result {
        self.build()
    }
}

pub struct WitValueChildItemsBuilder<ParentBuilder: NodeBuilder> {
    builder: ParentBuilder,
    target_idx: TypeIndex,
    items: Vec<TypeIndex>,
}

impl<ParentBuilder: NodeBuilder> WitValueChildItemsBuilder<ParentBuilder> {
    fn new(builder: ParentBuilder, target_idx: TypeIndex) -> Self {
        Self { builder, target_idx, items: Vec::new() }
    }

    fn add_item(&mut self, item_type_index: i32) {
        self.items.push(item_type_index);
    }

    pub fn item(self) -> WitValueItemBuilder<ParentBuilder> {
        WitValueItemBuilder {
            child_items_builder: self
        }
    }

    pub fn finish(mut self) -> ParentBuilder::Result {
        self.builder.parent_builder().finish_seq(self.items, self.target_idx);
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

    fn variant(mut self, case_idx: u32) -> WitValueItemBuilder<WitValueItemBuilder<ParentBuilder>> {
        let variant_idx = self.parent_builder().add_variant(case_idx, -1);
        self.child_items_builder.add_item(variant_idx);
        WitValueItemBuilder {
            child_items_builder: WitValueChildItemsBuilder::new(self, variant_idx)
        }
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

    fn option_some(mut self) -> WitValueItemBuilder<Self> {
        let option_idx = self.parent_builder().add_option();
        self.child_items_builder.add_item(option_idx);
        WitValueItemBuilder {
            child_items_builder: WitValueChildItemsBuilder::new(self, option_idx)
        }
    }

    fn option_none(mut self) -> Self::Result {
        let option_idx = self.parent_builder().add_option();
        self.child_items_builder.add_item(option_idx);
        self.child_items_builder
    }

    fn result_ok(mut self) -> WitValueItemBuilder<Self> {
        let result_idx = self.parent_builder().add_result_ok();
        self.child_items_builder.add_item(result_idx);
        WitValueItemBuilder {
            child_items_builder: WitValueChildItemsBuilder::new(self, result_idx)
        }
    }

    fn result_err(mut self) -> WitValueItemBuilder<Self> {
        let result_idx = self.parent_builder().add_result_err();
        self.child_items_builder.add_item(result_idx);
        WitValueItemBuilder {
            child_items_builder: WitValueChildItemsBuilder::new(self, result_idx)
        }
    }

    fn finish(self) -> Self::Result {
        self.child_items_builder
    }
}