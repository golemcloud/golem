use crate::rib::expr::Expr;
use crate::rib::parser::identifier::identifier;
use crate::rib::parser::literal::literal;
use crate::rib::parser::not::not;
use crate::rib::parser::sequence::sequence;
use combine::parser;
use combine::parser::char;
use combine::parser::choice::choice;
use combine::{attempt, Parser, Stream};

use super::binary_comparison::{
    equal_to, greater_than, greater_than_or_equal_to, less_than, less_than_or_equal_to,
};
use super::cond::conditional;
use super::flag::flag;
use super::let_binding::let_binding;
use super::optional::option;
use super::pattern_match::pattern_match;
use super::record::record;
use super::result::result;
use super::select_field::select_field;
use super::tuple::tuple;
use crate::rib::parser::call::call;
use combine::stream::easy;

// Exposed another function to handle recursion - based on docs
// Also note that, the immediate parsers on the sides of a binary expression can result in stack overflow
// Therefore we copy the parser without these binary parsers in the attempt list to build the  binary comparison parsers.
// This may not be intuitive however will work!

// Consider this as part of the  slight cost we pay in comparison  to building our own lexers and parsers.
// Example when to describe the consumption vs when to consume between tokens, safe mutual recursion to handle interpolation etc
// On the other hand building our own lexers and parsers can come up with bugs as well.
pub fn rib_expr_<'t>() -> impl Parser<easy::Stream<&'t str>, Output = Expr> {
    choice((
        attempt(flag()),
        attempt(pattern_match()),
        attempt(let_binding()),
        attempt(record()),
        attempt(tuple()),
        attempt(sequence()),
        attempt(literal()),
        attempt(not()),
        attempt(conditional()),
        attempt(option()),
        attempt(result()),
        attempt(greater_than_or_equal_to(
            rib_expr_without_binary(),
            rib_expr_without_binary(),
        )),
        attempt(greater_than(
            rib_expr_without_binary(),
            rib_expr_without_binary(),
        )),
        attempt(less_than_or_equal_to(
            rib_expr_without_binary(),
            rib_expr_without_binary(),
        )),
        attempt(less_than(
            rib_expr_without_binary(),
            rib_expr_without_binary(),
        )),
        attempt(equal_to(
            rib_expr_without_binary(),
            rib_expr_without_binary(),
        )),
        attempt(call()),
               attempt(identifier()),
    ))
}

pub fn rib_expr_without_binary_<'t>() -> impl Parser<easy::Stream<&'t str>, Output = Expr> {
    choice((
        attempt(call()),
        attempt(flag()),
        attempt(pattern_match()),
        attempt(let_binding()),
        attempt(record()),
        attempt(sequence()),
        attempt(literal()),
        attempt(not()),
        attempt(conditional()),
        attempt(option()),
        attempt(result()),
        attempt(call()),
        attempt(identifier()),
    ))
}

// As this expression parser needs to be able to call itself recursively `impl Parser` can't
// be used on its own as that would cause an infinitely large type. We can avoid this by using
// the `parser!` macro which erases the inner type and the size of that type entirely which
// lets it be used recursively.
//
// (This macro does not use `impl Trait` which means it can be used in rust < 1.26 as well to
// emulate `impl Parser`)
parser! {
    pub fn rib_expr['t]()(easy::Stream<&'t str>) -> Expr
    where [
        easy::Stream<&'t str>: Stream<Token = char>,
    ]
    {
        rib_expr_()
    }
}

parser! {
    pub fn rib_expr_without_binary['t]()(easy::Stream<&'t str>) -> Expr
    where [
        easy::Stream<&'t str>: Stream<Token = char>,
    ]
    {
        rib_expr_without_binary_()
    }
}
