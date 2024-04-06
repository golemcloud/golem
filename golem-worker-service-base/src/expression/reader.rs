use crate::expression::Expr;
use crate::parser::expr_parser::ExprParser;
use crate::parser::GolemParser;
use crate::parser::ParseError;

pub fn read_expr(input: impl AsRef<str>) -> Result<Expr, ParseError> {
    let expr_parser = ExprParser {};
    expr_parser.parse(input.as_ref())
}
