// TODO: get rid of clones in the builder
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
    fn enum_value(self, value: &str) -> Self::Result;
    fn flags(self, values: Vec<bool>) -> Self::Result;

    fn record(self) -> WitValueRecordBuilder<Self>;
    fn variant(self, name: &str) -> WitValueItemBuilder<Self>;
    fn tuple(self) -> WitValueSeqBuilder<Self>;
    fn list(self) -> WitValueSeqBuilder<Self>;

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

    pub(crate) fn add_variant(&mut self, name: &str, target_idx: TypeIndex) -> TypeIndex {
        self.add(WitNode::VariantValue((name.to_string(), target_idx)))
    }

    pub(crate) fn add_enum_value(&mut self, value: &str) -> TypeIndex {
        self.add(WitNode::EnumValue(value.to_string()))
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

    pub(crate) fn finish_record(&mut self, fields: Vec<(String, TypeIndex)>, target_idx: TypeIndex) {
        if let WitNode::RecordValue(ref mut result_fields) = &mut self.nodes[target_idx as usize] {
            *result_fields = fields;
        } else {
            panic!("finish_record called on non-record node");
        }
    }

    pub(crate) fn finish_seq(&mut self, items: Vec<TypeIndex>, target_idx: TypeIndex) {
        match &mut self.nodes[target_idx as usize] {
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

    fn enum_value(mut self, value: &str) -> Self::Result {
        let _ = self.add_enum_value(value);
        self.build()
    }

    fn flags(mut self, values: Vec<bool>) -> Self::Result {
        let _ = self.add_flags(values);
        self.build()
    }

    fn record(mut self) -> WitValueRecordBuilder<WitValueBuilder> {
        let idx = self.add_record();
        WitValueRecordBuilder::new(self, idx)
    }

    fn variant(mut self, name: &str) -> WitValueItemBuilder<WitValueBuilder> {
        let variant_idx = self.add_variant(name, -1);
        WitValueItemBuilder {
            seq_builder: WitValueSeqBuilder::new(self, variant_idx)
        }
    }

    fn tuple(mut self) -> WitValueSeqBuilder<WitValueBuilder> {
        let tuple_idx = self.add_tuple();
        WitValueSeqBuilder::new(self, tuple_idx)
    }

    fn list(mut self) -> WitValueSeqBuilder<WitValueBuilder> {
        let tuple_idx = self.add_list();
        WitValueSeqBuilder::new(self, tuple_idx)
    }

    fn option_some(mut self) -> WitValueItemBuilder<Self> {
        let option_idx = self.add_option();
        WitValueItemBuilder {
            seq_builder: WitValueSeqBuilder::new(self, option_idx)
        }
    }

    fn option_none(mut self) -> Self::Result {
        let _ = self.add_option();
        self.build()
    }

    fn result_ok(mut self) -> WitValueItemBuilder<Self> {
        let result_idx = self.add_result_ok();
        WitValueItemBuilder {
            seq_builder: WitValueSeqBuilder::new(self, result_idx)
        }
    }

    fn result_err(mut self) -> WitValueItemBuilder<Self> {
        let result_idx = self.add_result_err();
        WitValueItemBuilder {
            seq_builder: WitValueSeqBuilder::new(self, result_idx)
        }
    }

    fn finish(self) -> Self::Result {
        self.build()
    }
}

pub struct WitValueRecordBuilder<ParentBuilder> {
    builder: ParentBuilder,
    target_idx: TypeIndex,
    fields: Vec<(String, TypeIndex)>,
}

impl<ParentBuilder: NodeBuilder> WitValueRecordBuilder<ParentBuilder> {
    fn new(builder: ParentBuilder, target_idx: TypeIndex) -> Self {
        Self { builder, target_idx, fields: Vec::new() }
    }

    fn add_field(&mut self, field_name: String, field_type_index: i32) {
        self.fields.push((field_name, field_type_index));
    }

    pub fn field(self, field_name: &str) -> WitValueRecordFieldBuilder<ParentBuilder> {
        WitValueRecordFieldBuilder {
            record_builder: self,
            field_name: field_name.to_string(),
        }
    }

    pub fn finish(mut self) -> ParentBuilder::Result {
        self.builder.parent_builder().finish_record(self.fields, self.target_idx);
        self.builder.finish()
    }
}

pub struct WitValueRecordFieldBuilder<ParentBuilder: NodeBuilder> {
    record_builder: WitValueRecordBuilder<ParentBuilder>,
    field_name: String,
}

impl<ParentBuilder: NodeBuilder> NodeBuilder for WitValueRecordFieldBuilder<ParentBuilder> {
    type Result = WitValueRecordBuilder<ParentBuilder>;

    fn parent_builder(&mut self) -> &mut WitValueBuilder {
        self.record_builder.builder.parent_builder()
    }


    fn u8(mut self, value: u8) -> Self::Result {
        let field_type_index = self.parent_builder().add_u8(value);
        self.record_builder.add_field(self.field_name, field_type_index);
        self.record_builder
    }

    fn u16(mut self, value: u16) -> Self::Result {
        let field_type_index = self.parent_builder().add_u16(value);
        self.record_builder.add_field(self.field_name, field_type_index);
        self.record_builder
    }

    fn u32(mut self, value: u32) -> Self::Result {
        let field_type_index = self.parent_builder().add_u32(value);
        self.record_builder.add_field(self.field_name, field_type_index);
        self.record_builder
    }

    fn u64(mut self, value: u64) -> Self::Result {
        let field_type_index = self.parent_builder().add_u64(value);
        self.record_builder.add_field(self.field_name, field_type_index);
        self.record_builder
    }

    fn s8(mut self, value: i8) -> Self::Result {
        let field_type_index = self.parent_builder().add_s8(value);
        self.record_builder.add_field(self.field_name, field_type_index);
        self.record_builder
    }

    fn s16(mut self, value: i16) -> Self::Result {
        let field_type_index = self.parent_builder().add_s16(value);
        self.record_builder.add_field(self.field_name, field_type_index);
        self.record_builder
    }

    fn s32(mut self, value: i32) -> Self::Result {
        let field_type_index = self.parent_builder().add_s32(value);
        self.record_builder.add_field(self.field_name, field_type_index);
        self.record_builder
    }

    fn s64(mut self, value: i64) -> Self::Result {
        let field_type_index = self.parent_builder().add_s64(value);
        self.record_builder.add_field(self.field_name, field_type_index);
        self.record_builder
    }

    fn f32(mut self, value: f32) -> Self::Result {
        let field_type_index = self.parent_builder().add_f32(value);
        self.record_builder.add_field(self.field_name, field_type_index);
        self.record_builder
    }

    fn f64(mut self, value: f64) -> Self::Result {
        let field_type_index = self.parent_builder().add_f64(value);
        self.record_builder.add_field(self.field_name, field_type_index);
        self.record_builder
    }

    fn char(mut self, value: char) -> Self::Result {
        let field_type_index = self.parent_builder().add_char(value);
        self.record_builder.add_field(self.field_name, field_type_index);
        self.record_builder
    }

    fn bool(mut self, value: bool) -> Self::Result {
        let field_type_index = self.parent_builder().add_bool(value);
        self.record_builder.add_field(self.field_name, field_type_index);
        self.record_builder
    }

    fn string(mut self, value: &str) -> Self::Result {
        let field_type_index = self.parent_builder().add_string(value);
        self.record_builder.add_field(self.field_name, field_type_index);
        self.record_builder
    }

    fn enum_value(mut self, value: &str) -> Self::Result {
        let field_type_index = self.parent_builder().add_enum_value(value);
        self.record_builder.add_field(self.field_name, field_type_index);
        self.record_builder
    }

    fn flags(mut self, values: Vec<bool>) -> Self::Result {
        let field_type_index = self.parent_builder().add_flags(values);
        self.record_builder.add_field(self.field_name, field_type_index);
        self.record_builder
    }

    fn record(mut self) -> WitValueRecordBuilder<WitValueRecordFieldBuilder<ParentBuilder>> {
        let target_idx = self.parent_builder().add_record();
        self.record_builder.add_field(self.field_name.clone(), target_idx);
        WitValueRecordBuilder::new(self, target_idx)
    }

    fn variant(mut self, name: &str) -> WitValueItemBuilder<WitValueRecordFieldBuilder<ParentBuilder>> {
        let variant_idx = self.parent_builder().add_variant(name, -1);
        self.record_builder.add_field(self.field_name.clone(), variant_idx);
        WitValueItemBuilder {
            seq_builder: WitValueSeqBuilder::new(self, variant_idx)
        }
    }

    fn tuple(mut self) -> WitValueSeqBuilder<WitValueRecordFieldBuilder<ParentBuilder>> {
        let target_idx = self.parent_builder().add_tuple();
        self.record_builder.add_field(self.field_name.clone(), target_idx);
        WitValueSeqBuilder::new(self, target_idx)
    }

    fn list(mut self) -> WitValueSeqBuilder<WitValueRecordFieldBuilder<ParentBuilder>> {
        let target_idx = self.parent_builder().add_list();
        self.record_builder.add_field(self.field_name.clone(), target_idx);
        WitValueSeqBuilder::new(self, target_idx)
    }

    fn option_some(mut self) -> WitValueItemBuilder<Self> {
        let option_idx = self.parent_builder().add_option();
        self.record_builder.add_field(self.field_name.clone(), option_idx);
        WitValueItemBuilder {
            seq_builder: WitValueSeqBuilder::new(self, option_idx)
        }
    }

    fn option_none(mut self) -> Self::Result {
        let option_idx = self.parent_builder().add_option();
        self.record_builder.add_field(self.field_name, option_idx);
        self.record_builder
    }

    fn result_ok(mut self) -> WitValueItemBuilder<Self> {
        let result_idx = self.parent_builder().add_result_ok();
        self.record_builder.add_field(self.field_name.clone(), result_idx);
        WitValueItemBuilder {
            seq_builder: WitValueSeqBuilder::new(self, result_idx)
        }
    }

    fn result_err(mut self) -> WitValueItemBuilder<Self> {
        let result_idx = self.parent_builder().add_result_err();
        self.record_builder.add_field(self.field_name.clone(), result_idx);
        WitValueItemBuilder {
            seq_builder: WitValueSeqBuilder::new(self, result_idx)
        }
    }

    fn finish(self) -> Self::Result {
        self.record_builder
    }
}

pub struct WitValueSeqBuilder<ParentBuilder: NodeBuilder> {
    builder: ParentBuilder,
    target_idx: TypeIndex,
    items: Vec<TypeIndex>,
}

impl<ParentBuilder: NodeBuilder> WitValueSeqBuilder<ParentBuilder> {
    fn new(builder: ParentBuilder, target_idx: TypeIndex) -> Self {
        Self { builder, target_idx, items: Vec::new() }
    }

    fn add_item(&mut self, item_type_index: i32) {
        self.items.push(item_type_index);
    }

    pub fn item(self) -> WitValueItemBuilder<ParentBuilder> {
        WitValueItemBuilder {
            seq_builder: self
        }
    }

    pub fn finish(mut self) -> ParentBuilder::Result {
        self.builder.parent_builder().finish_seq(self.items, self.target_idx);
        self.builder.finish()
    }
}

pub struct WitValueItemBuilder<ParentBuilder: NodeBuilder> {
    seq_builder: WitValueSeqBuilder<ParentBuilder>,
}

impl<ParentBuilder: NodeBuilder> NodeBuilder for WitValueItemBuilder<ParentBuilder> {
    type Result = WitValueSeqBuilder<ParentBuilder>;

    fn parent_builder(&mut self) -> &mut WitValueBuilder {
        self.seq_builder.builder.parent_builder()
    }

    fn u8(mut self, value: u8) -> Self::Result {
        let item_type_index = self.parent_builder().add_u8(value);
        self.seq_builder.add_item(item_type_index);
        self.seq_builder
    }

    fn u16(mut self, value: u16) -> Self::Result {
        let item_type_index = self.parent_builder().add_u16(value);
        self.seq_builder.add_item(item_type_index);
        self.seq_builder
    }

    fn u32(mut self, value: u32) -> Self::Result {
        let item_type_index = self.parent_builder().add_u32(value);
        self.seq_builder.add_item(item_type_index);
        self.seq_builder
    }

    fn u64(mut self, value: u64) -> Self::Result {
        let item_type_index = self.parent_builder().add_u64(value);
        self.seq_builder.add_item(item_type_index);
        self.seq_builder
    }

    fn s8(mut self, value: i8) -> Self::Result {
        let item_type_index = self.parent_builder().add_s8(value);
        self.seq_builder.add_item(item_type_index);
        self.seq_builder
    }

    fn s16(mut self, value: i16) -> Self::Result {
        let item_type_index = self.parent_builder().add_s16(value);
        self.seq_builder.add_item(item_type_index);
        self.seq_builder
    }

    fn s32(mut self, value: i32) -> Self::Result {
        let item_type_index = self.parent_builder().add_s32(value);
        self.seq_builder.add_item(item_type_index);
        self.seq_builder
    }

    fn s64(mut self, value: i64) -> Self::Result {
        let item_type_index = self.parent_builder().add_s64(value);
        self.seq_builder.add_item(item_type_index);
        self.seq_builder
    }

    fn f32(mut self, value: f32) -> Self::Result {
        let item_type_index = self.parent_builder().add_f32(value);
        self.seq_builder.add_item(item_type_index);
        self.seq_builder
    }

    fn f64(mut self, value: f64) -> Self::Result {
        let item_type_index = self.parent_builder().add_f64(value);
        self.seq_builder.add_item(item_type_index);
        self.seq_builder
    }

    fn char(mut self, value: char) -> Self::Result {
        let item_type_index = self.parent_builder().add_char(value);
        self.seq_builder.add_item(item_type_index);
        self.seq_builder
    }

    fn bool(mut self, value: bool) -> Self::Result {
        let item_type_index = self.parent_builder().add_bool(value);
        self.seq_builder.add_item(item_type_index);
        self.seq_builder
    }

    fn string(mut self, value: &str) -> Self::Result {
        let item_type_index = self.parent_builder().add_string(value);
        self.seq_builder.add_item(item_type_index);
        self.seq_builder
    }

    fn enum_value(mut self, value: &str) -> Self::Result {
        let item_type_index = self.parent_builder().add_enum_value(value);
        self.seq_builder.add_item(item_type_index);
        self.seq_builder
    }

    fn flags(mut self, values: Vec<bool>) -> Self::Result {
        let item_type_index = self.parent_builder().add_flags(values);
        self.seq_builder.add_item(item_type_index);
        self.seq_builder
    }

    fn record(mut self) -> WitValueRecordBuilder<WitValueItemBuilder<ParentBuilder>> {
        let target_idx = self.parent_builder().add_record();
        self.seq_builder.add_item(target_idx);
        WitValueRecordBuilder::new(self, target_idx)
    }

    fn variant(mut self, name: &str) -> WitValueItemBuilder<WitValueItemBuilder<ParentBuilder>> {
        let variant_idx = self.parent_builder().add_variant(name, -1);
        self.seq_builder.add_item(variant_idx);
        WitValueItemBuilder {
            seq_builder: WitValueSeqBuilder::new(self, variant_idx)
        }
    }

    fn tuple(mut self) -> WitValueSeqBuilder<Self> {
        let target_idx = self.parent_builder().add_tuple();
        self.seq_builder.add_item(target_idx);
        WitValueSeqBuilder::new(self, target_idx)
    }

    fn list(mut self) -> WitValueSeqBuilder<Self> {
        let target_idx = self.parent_builder().add_list();
        self.seq_builder.add_item(target_idx);
        WitValueSeqBuilder::new(self, target_idx)
    }

    fn option_some(mut self) -> WitValueItemBuilder<Self> {
        let option_idx = self.parent_builder().add_option();
        self.seq_builder.add_item(option_idx);
        WitValueItemBuilder {
            seq_builder: WitValueSeqBuilder::new(self, option_idx)
        }
    }

    fn option_none(mut self) -> Self::Result {
        let option_idx = self.parent_builder().add_option();
        self.seq_builder.add_item(option_idx);
        self.seq_builder
    }

    fn result_ok(mut self) -> WitValueItemBuilder<Self> {
        let result_idx = self.parent_builder().add_result_ok();
        self.seq_builder.add_item(result_idx);
        WitValueItemBuilder {
            seq_builder: WitValueSeqBuilder::new(self, result_idx)
        }
    }

    fn result_err(mut self) -> WitValueItemBuilder<Self> {
        let result_idx = self.parent_builder().add_result_err();
        self.seq_builder.add_item(result_idx);
        WitValueItemBuilder {
            seq_builder: WitValueSeqBuilder::new(self, result_idx)
        }
    }

    fn finish(self) -> Self::Result {
        self.seq_builder
    }
}