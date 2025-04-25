use crate::{InferredType, TypeInternal};
use std::collections::HashSet;

// Convert AllOf(AllOf(x, y, z), AllOf(a, b, OneOf(c, d))) to AllOf(x, y, z, a, b, OneOf(c,d))
// In Rib inference, there is no situation of a OneOf having AllOf
// We intentionally make sure we have only AllOf(OneOf) and not OneOf(AllOf)
pub fn flatten_all_of_list(types: &Vec<InferredType>) -> Vec<InferredType> {
    let mut all_of_types = vec![];
    let mut seen = HashSet::new();

    for typ in types {
        match typ.inner.as_ref() {
            TypeInternal::AllOf(all_of) => {
                let flattened = flatten_all_of_list(all_of);
                for t in flattened {
                    if seen.insert(t.clone()) {
                        all_of_types.push(t);
                    }
                }
            }
            _ => {
                all_of_types.push(typ.clone());
            }
        }
    }

    all_of_types
}
