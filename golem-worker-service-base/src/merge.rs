use golem_wasm_rpc::TypeAnnotatedValue;

pub trait Merge {
    fn merge(&self, other: &Self) -> Self;
}

impl Merge for TypeAnnotatedValue {
    // the only way to merge two type annotated value is only if they are records
    fn merge(&self, other: &Self) -> Self {
        match (self, other) {
            (
                TypeAnnotatedValue::Record { value, typ },
                TypeAnnotatedValue::Record {
                    value: other_value,
                    typ: other_typ,
                },
            ) => {
                let mut new_value = value.clone();
                let mut types = typ.clone();
                for (key, value) in other_value {
                    new_value.push((key.clone(), value.clone()));
                }
                for (key, value) in other_typ {
                    types.push((key.clone(), value.clone()));
                }
                TypeAnnotatedValue::Record {
                    typ: types,
                    value: new_value,
                }
            }
            _ => self.clone(),
        }
    }
}
