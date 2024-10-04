use rib::{type_pull_up_non_recursive, Expr, InferredType};

fn main() {
    let record_identifier = Expr::identifier("foo").add_infer_type(InferredType::Record(vec![(
        "foo".to_string(),
        InferredType::Record(vec![("bar".to_string(), InferredType::U64)]),
    )]));
    let select_expr = Expr::select_field(record_identifier, "foo");
    let new_expr = type_pull_up_non_recursive(&select_expr);
    dbg!(new_expr);
}
