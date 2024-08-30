    use crate::parser::type_name::TypeName;
    use crate::{Expr, InferredType};

    pub(crate) fn bind(expr: &Expr, type_name: Option<TypeName>) -> Expr {
        if let Some(type_name) = type_name {
            let mut expr = expr.clone();
            override_type(&mut expr, type_name.into());
            expr
        } else {
            expr.clone()
        }
    }

    pub(crate) fn override_type(expr: &mut Expr, new_type: InferredType) {
        match expr {
            Expr::Identifier(_, inferred_type)
            | Expr::Let(_, _, inferred_type)
            | Expr::SelectField(_, _, inferred_type)
            | Expr::SelectIndex(_, _, inferred_type)
            | Expr::Sequence(_, inferred_type)
            | Expr::Record(_, inferred_type)
            | Expr::Tuple(_, inferred_type)
            | Expr::Literal(_, inferred_type)
            | Expr::Number(_, inferred_type)
            | Expr::Flags(_, inferred_type)
            | Expr::Boolean(_, inferred_type)
            | Expr::Concat(_, inferred_type)
            | Expr::Multiple(_, inferred_type)
            | Expr::Not(_, inferred_type)
            | Expr::GreaterThan(_, _, inferred_type)
            | Expr::GreaterThanOrEqualTo(_, _, inferred_type)
            | Expr::LessThanOrEqualTo(_, _, inferred_type)
            | Expr::EqualTo(_, _, inferred_type)
            | Expr::LessThan(_, _, inferred_type)
            | Expr::Cond(_, _, _, inferred_type)
            | Expr::PatternMatch(_, _, inferred_type)
            | Expr::Option(_, inferred_type)
            | Expr::Result(_, inferred_type)
            | Expr::Unwrap(_, inferred_type)
            | Expr::Throw(_, inferred_type)
            | Expr::Tag(_, inferred_type)
            | Expr::Call(_, _, inferred_type) => {
                *inferred_type = new_type;
            }
        }
    }