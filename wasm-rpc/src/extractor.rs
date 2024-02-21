use crate::{Uri, WitNode, WitValue};

pub trait WitValueExtractor<'a> {
    fn u8(&'a self) -> Option<u8>;
    fn u16(&'a self) -> Option<u16>;
    fn u32(&'a self) -> Option<u32>;
    fn u64(&'a self) -> Option<u64>;
    fn s8(&'a self) -> Option<i8>;
    fn s16(&'a self) -> Option<i16>;
    fn s32(&'a self) -> Option<i32>;
    fn s64(&'a self) -> Option<i64>;
    fn f32(&'a self) -> Option<f32>;
    fn f64(&'a self) -> Option<f64>;
    fn char(&'a self) -> Option<char>;
    fn bool(&'a self) -> Option<bool>;
    fn string(&'a self) -> Option<&'a str>;
    fn field(&'a self, field_idx: usize) -> Option<WitNodePointer<'a>>;
    fn variant(&'a self) -> Option<(u32, Option<WitNodePointer<'a>>)>;
    fn enum_value(&'a self) -> Option<u32>;
    fn flags(&'a self) -> Option<&'a [bool]>;
    fn tuple_element(&'a self, element_idx: usize) -> Option<WitNodePointer<'a>>;
    fn list_elements<R>(&'a self, f: impl Fn(WitNodePointer<'a>) -> R) -> Option<Vec<R>>;
    fn option(&'a self) -> Option<Option<WitNodePointer<'a>>>;
    fn result(&'a self) -> Option<Result<Option<WitNodePointer<'a>>, Option<WitNodePointer<'a>>>>;

    fn handle(&'a self) -> Option<(Uri, u64)>;
}

impl<'a> WitValueExtractor<'a> for WitValue {
    fn u8(&self) -> Option<u8> {
        WitNodePointer::new(self, 0).u8()
    }

    fn u16(&self) -> Option<u16> {
        WitNodePointer::new(self, 0).u16()
    }

    fn u32(&self) -> Option<u32> {
        WitNodePointer::new(self, 0).u32()
    }

    fn u64(&self) -> Option<u64> {
        WitNodePointer::new(self, 0).u64()
    }

    fn s8(&self) -> Option<i8> {
        WitNodePointer::new(self, 0).s8()
    }

    fn s16(&self) -> Option<i16> {
        WitNodePointer::new(self, 0).s16()
    }

    fn s32(&self) -> Option<i32> {
        WitNodePointer::new(self, 0).s32()
    }

    fn s64(&self) -> Option<i64> {
        WitNodePointer::new(self, 0).s64()
    }

    fn f32(&self) -> Option<f32> {
        WitNodePointer::new(self, 0).f32()
    }

    fn f64(&self) -> Option<f64> {
        WitNodePointer::new(self, 0).f64()
    }

    fn char(&self) -> Option<char> {
        WitNodePointer::new(self, 0).char()
    }

    fn bool(&self) -> Option<bool> {
        WitNodePointer::new(self, 0).bool()
    }

    fn string(&'a self) -> Option<&'a str> {
        WitNodePointer::<'a>::new(self, 0).string()
    }

    fn field(&'a self, field_idx: usize) -> Option<WitNodePointer<'a>> {
        WitNodePointer::new(self, 0).field(field_idx)
    }

    fn variant(&'a self) -> Option<(u32, Option<WitNodePointer<'a>>)> {
        WitNodePointer::new(self, 0).variant()
    }

    fn enum_value(&'a self) -> Option<u32> {
        WitNodePointer::new(self, 0).enum_value()
    }

    fn flags(&'a self) -> Option<&'a [bool]> {
        WitNodePointer::new(self, 0).flags()
    }

    fn tuple_element(&'a self, element_idx: usize) -> Option<WitNodePointer<'a>> {
        WitNodePointer::new(self, 0).tuple_element(element_idx)
    }

    fn list_elements<R>(&'a self, f: impl Fn(WitNodePointer<'a>) -> R) -> Option<Vec<R>> {
        WitNodePointer::new(self, 0).list_elements(f)
    }

    fn option(&'a self) -> Option<Option<WitNodePointer<'a>>> {
        WitNodePointer::new(self, 0).option()
    }

    fn result(&'a self) -> Option<Result<Option<WitNodePointer<'a>>, Option<WitNodePointer<'a>>>> {
        WitNodePointer::new(self, 0).result()
    }

    fn handle(&'a self) -> Option<(Uri, u64)> {
        WitNodePointer::new(self, 0).handle()
    }
}

pub struct WitNodePointer<'a> {
    value: &'a WitValue,
    idx: usize,
}

impl<'a> WitNodePointer<'a> {
    fn new(value: &'a WitValue, idx: usize) -> Self {
        assert!(idx < value.nodes.len());
        Self { value, idx }
    }

    fn node(&self) -> &'a WitNode {
        &self.value.nodes[self.idx]
    }

    pub fn u8(&self) -> Option<u8> {
        if let WitNode::PrimU8(value) = self.node() {
            Some(*value)
        } else {
            None
        }
    }

    pub fn u16(&self) -> Option<u16> {
        if let WitNode::PrimU16(value) = self.node() {
            Some(*value)
        } else {
            None
        }
    }

    pub fn u32(&self) -> Option<u32> {
        if let WitNode::PrimU32(value) = self.node() {
            Some(*value)
        } else {
            None
        }
    }

    pub fn u64(&self) -> Option<u64> {
        if let WitNode::PrimU64(value) = self.node() {
            Some(*value)
        } else {
            None
        }
    }

    pub fn s8(&self) -> Option<i8> {
        if let WitNode::PrimS8(value) = self.node() {
            Some(*value)
        } else {
            None
        }
    }

    pub fn s16(&self) -> Option<i16> {
        if let WitNode::PrimS16(value) = self.node() {
            Some(*value)
        } else {
            None
        }
    }

    pub fn s32(&self) -> Option<i32> {
        if let WitNode::PrimS32(value) = self.node() {
            Some(*value)
        } else {
            None
        }
    }

    pub fn s64(&self) -> Option<i64> {
        if let WitNode::PrimS64(value) = self.node() {
            Some(*value)
        } else {
            None
        }
    }

    pub fn f32(&self) -> Option<f32> {
        if let WitNode::PrimFloat32(value) = self.node() {
            Some(*value)
        } else {
            None
        }
    }

    pub fn f64(&self) -> Option<f64> {
        if let WitNode::PrimFloat64(value) = self.node() {
            Some(*value)
        } else {
            None
        }
    }

    pub fn char(&self) -> Option<char> {
        if let WitNode::PrimChar(value) = self.node() {
            Some(*value)
        } else {
            None
        }
    }

    pub fn bool(&self) -> Option<bool> {
        if let WitNode::PrimBool(value) = self.node() {
            Some(*value)
        } else {
            None
        }
    }

    pub fn string(&self) -> Option<&'a str> {
        if let WitNode::PrimString(value) = self.node() {
            Some(value)
        } else {
            None
        }
    }

    pub fn field(&self, field_idx: usize) -> Option<WitNodePointer<'a>> {
        if let WitNode::RecordValue(fields) = self.node() {
            fields
                .get(field_idx)
                .map(|idx| WitNodePointer::new(self.value, *idx as usize))
        } else {
            None
        }
    }

    pub fn variant(&self) -> Option<(u32, Option<WitNodePointer<'a>>)> {
        if let WitNode::VariantValue((case, value)) = self.node() {
            let value = value.map(|idx| WitNodePointer::new(self.value, idx as usize));
            Some((*case, value))
        } else {
            None
        }
    }

    pub fn enum_value(&self) -> Option<u32> {
        if let WitNode::EnumValue(value) = self.node() {
            Some(*value)
        } else {
            None
        }
    }

    pub fn flags(&self) -> Option<&'a [bool]> {
        if let WitNode::FlagsValue(value) = self.node() {
            Some(value)
        } else {
            None
        }
    }

    pub fn tuple_element(&self, element_idx: usize) -> Option<WitNodePointer<'a>> {
        if let WitNode::TupleValue(elements) = self.node() {
            elements
                .get(element_idx)
                .map(|idx| WitNodePointer::new(self.value, *idx as usize))
        } else {
            None
        }
    }

    fn list_elements<R>(&self, f: impl Fn(WitNodePointer<'a>) -> R) -> Option<Vec<R>> {
        if let WitNode::ListValue(elements) = self.node() {
            Some(
                elements
                    .iter()
                    .map(|idx| f(WitNodePointer::new(self.value, *idx as usize)))
                    .collect(),
            )
        } else {
            None
        }
    }

    pub fn option(&self) -> Option<Option<WitNodePointer<'a>>> {
        if let WitNode::OptionValue(value) = self.node() {
            Some(value.map(|idx| WitNodePointer::new(self.value, idx as usize)))
        } else {
            None
        }
    }

    pub fn result(&self) -> Option<Result<Option<WitNodePointer<'a>>, Option<WitNodePointer<'a>>>> {
        if let WitNode::ResultValue(value) = self.node() {
            Some(match value {
                Ok(idx) => Ok(idx.map(|idx| WitNodePointer::new(self.value, idx as usize))),
                Err(idx) => Err(idx.map(|idx| WitNodePointer::new(self.value, idx as usize))),
            })
        } else {
            None
        }
    }

    pub fn handle(&self) -> Option<(Uri, u64)> {
        if let WitNode::Handle((uri, idx)) = self.node() {
            Some((uri.clone(), *idx))
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::*;

    #[test]
    fn primitive() {
        let value = WitValue::builder().s32(42);
        assert_eq!(value.s32(), Some(42));
    }

    #[test]
    fn single_record() {
        let value = WitValue::builder()
            .record()
            .item()
            .u8(1)
            .item()
            .enum_value(2)
            .item()
            .flags(vec![true, false, true])
            .finish();
        assert_eq!(value.field(0).unwrap().u8(), Some(1));
        assert_eq!(value.field(1).unwrap().enum_value(), Some(2));
        assert_eq!(
            value.field(2).unwrap().flags().unwrap(),
            &[true, false, true]
        );
    }

    #[test]
    fn deep_record() {
        let value = WitValue::builder()
            .record()
            .item()
            .record()
            .item()
            .s32(10)
            .item()
            .string("hello")
            .finish()
            .finish();

        let inner = value.field(0).unwrap();

        assert_eq!(inner.field(0).unwrap().s32(), Some(10));
        assert_eq!(inner.field(1).unwrap().string(), Some("hello"));
    }

    #[test]
    fn variant1() {
        let value = WitValue::builder().variant(2).s32(42).finish();
        assert_eq!(value.variant().unwrap().0, 2);
        assert_eq!(value.variant().unwrap().1.unwrap().s32(), Some(42));
    }

    #[test]
    fn variant2() {
        let value = WitValue::builder().variant_unit(0);
        assert_eq!(value.variant().unwrap().0, 0);
        assert!(value.variant().unwrap().1.is_none());
    }

    #[test]
    fn enum1() {
        let value = WitValue::builder().enum_value(2);
        assert_eq!(value.enum_value(), Some(2));
    }

    #[test]
    fn flags() {
        let value = WitValue::builder().flags(vec![true, false, true]);
        assert_eq!(value.flags().unwrap(), &[true, false, true]);
    }

    #[test]
    fn tuple() {
        let value = WitValue::builder()
            .tuple()
            .item()
            .s32(42)
            .item()
            .string("hello")
            .item()
            .record()
            .item()
            .string("world")
            .finish()
            .finish();
        assert_eq!(value.tuple_element(0).unwrap().s32(), Some(42));
        assert_eq!(value.tuple_element(1).unwrap().string(), Some("hello"));
        assert_eq!(
            value.tuple_element(2).unwrap().field(0).unwrap().string(),
            Some("world")
        );
    }

    #[test]
    fn list() {
        let value =
            WitValue::builder().list_fn(&[1, 2, 3, 4], |n, item_builder| item_builder.s32(*n));

        assert_eq!(
            value.list_elements(|v| v.s32().unwrap()).unwrap(),
            vec![1, 2, 3, 4]
        );
    }

    #[test]
    fn option1() {
        let value = WitValue::builder().option_none();
        assert!(value.option().unwrap().is_none());
    }

    #[test]
    fn option2() {
        let value = WitValue::builder().option_some().s32(42).finish();
        assert_eq!(value.option().unwrap().unwrap().s32(), Some(42));
    }

    #[test]
    fn result1() {
        let value = WitValue::builder().result_ok().s32(42).finish();
        assert_eq!(
            value.result().unwrap().ok().unwrap().unwrap().s32(),
            Some(42)
        );
    }

    #[test]
    fn result2() {
        let value = WitValue::builder().result_err().s32(42).finish();
        assert_eq!(
            value.result().unwrap().err().unwrap().unwrap().s32(),
            Some(42)
        );
    }

    #[test]
    fn result3() {
        let value = WitValue::builder().result_ok_unit();
        assert!(value.result().unwrap().ok().unwrap().is_none());
    }

    #[test]
    fn result4() {
        let value = WitValue::builder().result_err_unit();
        assert!(value.result().unwrap().err().unwrap().is_none());
    }

    #[test]
    fn handle() {
        let value = WitValue::builder().handle(
            Uri {
                value: "wit://test".to_string(),
            },
            42,
        );
        assert_eq!(
            value.handle().unwrap(),
            (
                Uri {
                    value: "wit://test".to_string()
                },
                42
            )
        );
    }
}
