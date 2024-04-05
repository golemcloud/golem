use golem_wasm_rpc::TypeAnnotatedValue;
pub(crate) trait Merge {
    fn merge(&mut self, other: &Self) -> &mut Self;
}

impl Merge for TypeAnnotatedValue {
    fn merge(&mut self, other: &Self) -> &mut Self {
        match (&mut *self, other) {
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

                *value = new_value;
                *typ = types;

                self
            }
            _ => self
        }
    }
}
