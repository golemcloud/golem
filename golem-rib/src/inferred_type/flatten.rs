use crate::InferredType;

// Convert AllOf(AllOf(x, y, z), AllOf(a, b, OneOf(c, d))) to AllOf(x, y, z, a, b, OneOf(c,d))
// In Rib inference, there is no situation of a OneOf having AllOf
// We intentionally make sure we have only AllOf(OneOf) and not OneOf(AllOf)
pub fn flatten_all_of_list(types: &Vec<InferredType>) -> Vec<InferredType> {
    let mut one_of_types = vec![];
    let mut all_of_types = vec![];

    for typ in types {
        match typ {
            InferredType::OneOf(types) => {
                let flattened = flatten_one_of_list(types);
                one_of_types.extend(flattened);
            }
            InferredType::AllOf(all_of) => {
                let flattened = flatten_all_of_list(all_of);
                all_of_types.extend(flattened);
            }
            _ => {
                all_of_types.push(typ.clone());
            }
        }
    }

    if !one_of_types.is_empty() {
        all_of_types.extend(vec![InferredType::OneOf(one_of_types)]);
    }

    all_of_types
}

// Convert OneOf(OneOf(x, y, z), OneOf(a, b)) to OneOf(x, y, z)
// Note that we don't have the situation of OneOf(AllOf) in Rib inference.
// The simplest form of resolving a OneOf is adding information of AllOf in the outer layer.
// Otherwise, `OneOf` is unresolved forever.
pub fn flatten_one_of_list(types: &Vec<InferredType>) -> Vec<InferredType> {
    let mut one_of_types = vec![];
    let mut all_of_types = vec![];

    for typ in types {
        match typ {
            InferredType::OneOf(types) => {
                let flattened = flatten_one_of_list(types);

                one_of_types.extend(flattened);
            }
            InferredType::AllOf(types) => {
                let flattened = flatten_all_of_list(types);
                all_of_types.extend(flattened);
            }
            _ => {
                one_of_types.push(typ.clone());
            }
        }
    }

    if !all_of_types.is_empty() {
        one_of_types.extend(vec![InferredType::AllOf(all_of_types)]);
    }

    one_of_types
}
