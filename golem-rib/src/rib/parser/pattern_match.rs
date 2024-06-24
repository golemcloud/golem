use match_arm::*;

use crate::rib::expr::Expr;
use crate::rib::parser::rib_expr::rib_expr;
use combine::parser::char::{char, spaces, string};
use combine::stream::easy;
use combine::{sep_by1, Parser};

pub fn pattern_match<'t>() -> impl Parser<easy::Stream<&'t str>, Output = Expr> {
    let arms = sep_by1(match_arm().skip(spaces()), char(',').skip(spaces()));

    (
        string("match").skip(spaces()),
        rib_expr().skip(spaces()),
        char('{').skip(spaces()),
        arms.skip(spaces()),
        char('}').skip(spaces()),
    )
        .map(|(_, expr, _, arms, _)| Expr::PatternMatch(Box::new(expr), arms))
}

mod match_arm {
    use combine::{easy, parser::char::string, Parser};

    use combine::parser::char::spaces;

    use super::arm_pattern::*;

    use crate::rib::expr::MatchArm;
    use crate::rib::parser::rib_expr::rib_expr;

    // RHS of a match arm
    pub(crate) fn match_arm<'t>() -> impl Parser<easy::Stream<&'t str>, Output = MatchArm> {
        (
            //LHS
            arm_pattern().skip(spaces()),
            string("=>").skip(spaces()),
            //RHS
            rib_expr().skip(spaces()),
        )
            .map(|(lhs, _, rhs)| MatchArm((lhs, Box::new(rhs))))
    }
}

// Keep the module structure same to avoid recursion related compiler errors
mod arm_pattern {
    use combine::{choice, parser, parser::char::char, Parser, Stream};

    use crate::rib::parser::pattern_match::internal::*;

    use crate::rib::expr::ArmPattern;

    use combine::attempt;

    use combine::stream::easy;

    // LHS of a match arm
    fn arm_pattern_<'t>() -> impl Parser<easy::Stream<&'t str>, Output = ArmPattern> {
        choice((
            attempt(arm_pattern_constructor()),
            attempt(char('_').map(|_| ArmPattern::WildCard)),
            attempt(arm_pattern_literal()),
        ))
    }

    parser! {
        pub(crate) fn arm_pattern['t]()(easy::Stream<&'t str>) -> ArmPattern
        where [easy::Stream<&'t str>: Stream<Token = char>,]{
            arm_pattern_()
        }
    }
}

mod internal {
    use combine::{choice, easy};
    use combine::{parser::char::char as char_, Parser};

    use crate::rib::expr::ConstructorTypeName;

    use crate::rib::expr::ArmPattern;
    use crate::rib::parser::optional::option;
    use crate::rib::parser::result::result;
    use crate::rib::parser::rib_expr::rib_expr;

    use crate::rib::parser::pattern_match::arm_pattern::*;
    use combine::attempt;
    use combine::many1;
    use combine::parser::char::letter;
    use combine::parser::char::{spaces, string};
    use combine::sep_by;

    pub(crate) fn arm_pattern_constructor<'t>(
    ) -> impl Parser<easy::Stream<&'t str>, Output = ArmPattern> {
        choice((
            attempt(option().map(|expr| ArmPattern::Literal(Box::new(expr)))),
            attempt(result().map(|expr| ArmPattern::Literal(Box::new(expr)))),
            attempt(custom_arm_pattern_constructor()),
        ))
    }

    pub(crate) fn arm_pattern_literal<'t>(
    ) -> impl Parser<easy::Stream<&'t str>, Output = ArmPattern> {
        rib_expr().map(|lit| ArmPattern::Literal(Box::new(lit)))
    }

    fn custom_arm_pattern_constructor<'t>(
    ) -> impl Parser<easy::Stream<&'t str>, Output = ArmPattern> {
        (
            constructor_type_name().skip(spaces()),
            string("(").skip(spaces()),
            sep_by(arm_pattern().skip(spaces()), char_(',')),
            string(")").skip(spaces()),
        )
            .map(|(name, _, patterns, _)| ArmPattern::Constructor(name, patterns))
    }

    fn constructor_type_name<'t>(
    ) -> impl Parser<easy::Stream<&'t str>, Output = ConstructorTypeName> {
        many1(letter().or(char_('_')))
            .map(|s: Vec<char>| s.into_iter().collect())
            .message("Unable to parse custom constructor name")
            .map(ConstructorTypeName::Identifier)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rib::expr::ArmPattern;
    use crate::rib::expr::ConstructorTypeName;
    use crate::rib::expr::Expr;
    use crate::rib::expr::MatchArm;
    use combine::EasyParser;

    #[test]
    fn test_simple_pattern_match() {
        let input = "match foo { _ => bar }";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::PatternMatch(
                    Box::new(Expr::Identifier("foo".to_string())),
                    vec![MatchArm((
                        ArmPattern::WildCard,
                        Box::new(Expr::Identifier("bar".to_string()))
                    ))]
                ),
                ""
            ))
        );
    }

    #[test]
    fn test_pattern_match_with_custom_constructor() {
        let input = "match foo { Foo(x) => bar }";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::PatternMatch(
                    Box::new(Expr::Identifier("foo".to_string())),
                    vec![MatchArm((
                        ArmPattern::Constructor(
                            ConstructorTypeName::Identifier("Foo".to_string()),
                            vec![ArmPattern::Literal(Box::new(Expr::Identifier(
                                "x".to_string()
                            )))]
                        ),
                        Box::new(Expr::Identifier("bar".to_string()))
                    ))]
                ),
                ""
            ))
        );
    }

    #[test]
    fn test_pattern_match() {
        let input = "match foo { _ => bar, ok(x) => x, err(x) => x, none => foo, some(x) => x }";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::PatternMatch(
                    Box::new(Expr::Identifier("foo".to_string())),
                    vec![
                        MatchArm((
                            ArmPattern::WildCard,
                            Box::new(Expr::Identifier("bar".to_string()))
                        )),
                        MatchArm((
                            ArmPattern::Literal(Box::new(Expr::Result(Ok(Box::new(
                                Expr::Identifier("x".to_string())
                            ))))),
                            Box::new(Expr::Identifier("x".to_string()))
                        )),
                        MatchArm((
                            ArmPattern::Literal(Box::new(Expr::Result(Err(Box::new(
                                Expr::Identifier("x".to_string())
                            ))))),
                            Box::new(Expr::Identifier("x".to_string()))
                        )),
                        MatchArm((
                            ArmPattern::Literal(Box::new(Expr::Option(None))),
                            Box::new(Expr::Identifier("foo".to_string()))
                        )),
                        MatchArm((
                            ArmPattern::Literal(Box::new(Expr::Option(Some(Box::new(
                                Expr::Identifier("x".to_string())
                            ))))),
                            Box::new(Expr::Identifier("x".to_string()))
                        )),
                    ]
                ),
                ""
            ))
        );
    }
}
