use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
use golem_wasm_rpc::protobuf::Record;
pub(crate) trait Merge {
    fn merge(&mut self, other: &Self) -> &mut Self;
}

impl Merge for TypeAnnotatedValue {
    fn merge(&mut self, other: &Self) -> &mut Self {
        match (&mut *self, other) {
            (
                TypeAnnotatedValue::Record (Record{ value, typ }),
                TypeAnnotatedValue::Record (Record {
                    value: other_value,
                    typ: other_typ,
                }),
            ) => {
                let mut new_value = value.clone();
                let mut types = typ.clone();

                for key_value in other_value {
                    new_value.push(key_value.clone());
                }

                for key_value in other_typ {
                    types.push(key_value.clone());
                }

                *value = new_value;
                *typ = types;

                self
            }
            _ => self,
        }
    }
}
