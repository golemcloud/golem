use crate::expression::Expr;
use crate::parser::expr_parser::parse_code;
use crate::parser::ParseError;
use crate::tokeniser::tokenizer::{Token, Tokenizer};

pub(crate) fn create_binary_op<F>(
    tokenizer: &mut Tokenizer,
    prev: Expr,
    get_expr: F,
) -> Result<Expr, ParseError>
where
    F: Fn(Box<Expr>, Box<Expr>) -> Expr,
{
    let right_op_str = tokenizer.capture_string_until(&Token::SemiColon).or_else(|| Some(tokenizer.consume_rest().to_string()))
        .ok_or::<ParseError>("Binary Op is applied to a non existing right expression".into())?
        .to_string();

    let right_op = parse_code(right_op_str)?;

    Ok(get_expr(Box::new(prev), Box::new(right_op)))
}
