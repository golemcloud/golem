use crate::type_checker::exhaustive_pattern_match::internal::ExhaustiveCheckError;
use crate::Expr;
use std::collections::VecDeque;

// When checking exhaustive pattern match, there is no need to ensure
// if the pattern aligns with conditions because those checks are done
// as part of previous phases of compilation. All we need to worry about
// is whether the arms in the pattern match is exhaustive.
pub fn check_exhaustive_pattern_match(expr: &mut Expr) -> Result<(), ExhaustiveCheckError> {
    let mut queue = VecDeque::new();
    queue.push_back(expr);

    while let Some(expr) = queue.pop_back() {
        match expr {
            Expr::PatternMatch(_, patterns, _) => {
                let match_arm = patterns
                    .iter()
                    .map(|p| p.arm_pattern.clone())
                    .collect::<Vec<_>>();
                internal::check_exhaustive_pattern_match(&match_arm)?;
            }

            expr => expr.visit_children_mut_bottom_up(&mut queue),
        }
    }

    Ok(())
}

mod internal {
    use crate::ArmPattern;
    use std::collections::HashMap;
    use std::fmt::Display;

    pub fn check_exhaustive_pattern_match(arms: &[ArmPattern]) -> Result<(), ExhaustiveCheckError> {
        let check_result =
            check_exhaustive(arms, &["none"], &["some"]).or(arms, &[], &["ok", "err"]);

        dbg!(check_result.clone());
        let inner_constructors = check_result.value()?;

        for (_, patterns) in inner_constructors.value() {
            check_exhaustive_pattern_match(patterns)?;
        }

        Ok(())
    }

    #[derive(Clone, Debug)]
    pub struct ConstructorPatterns(HashMap<String, Vec<ArmPattern>>);

    impl ConstructorPatterns {
        pub fn value(&self) -> &HashMap<String, Vec<ArmPattern>> {
            &self.0
        }

        fn is_empty(&self) -> bool {
            self.0.is_empty()
        }
    }

    #[derive(Debug, Clone)]
    pub struct ExhaustiveCheckResult(pub Result<ConstructorPatterns, ExhaustiveCheckError>);

    #[derive(Debug, Clone)]
    pub enum ExhaustiveCheckError {
        MissingConstructors(Vec<String>),
        DeadCode {
            cause: ArmPattern,
            dead_pattern: ArmPattern,
        },
    }

    impl ExhaustiveCheckResult {
        pub fn or(
            &self,
            patterns: &[ArmPattern],
            no_arg_constructors: &[&str],
            with_arg_constructors: &[&str],
        ) -> ExhaustiveCheckResult {
            match self {
                ExhaustiveCheckResult(Ok(result)) if result.is_empty() => {
                    check_exhaustive(patterns, no_arg_constructors, with_arg_constructors)
                }
                ExhaustiveCheckResult(Ok(_)) => self.clone(),
                ExhaustiveCheckResult(Err(e)) => ExhaustiveCheckResult(Err(e.clone())),
            }
        }

        pub fn value(&self) -> Result<ConstructorPatterns, ExhaustiveCheckError> {
            self.0.clone()
        }

        pub fn missing_constructors(missing_constructors: Vec<String>) -> Self {
            ExhaustiveCheckResult(Err(ExhaustiveCheckError::MissingConstructors(
                missing_constructors,
            )))
        }

        pub fn dead_code(cause: &ArmPattern, dead_pattern: &ArmPattern) -> Self {
            ExhaustiveCheckResult(Err(ExhaustiveCheckError::DeadCode {
                cause: cause.clone(),
                dead_pattern: dead_pattern.clone(),
            }))
        }

        pub fn succeed(constructor_patterns: ConstructorPatterns) -> Self {
            ExhaustiveCheckResult(Ok(constructor_patterns))
        }
    }

    impl Display for ExhaustiveCheckError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                ExhaustiveCheckError::MissingConstructors(constructors) => {
                    let constructors = constructors.join(", ");
                    write!(f, "Error: Non-exhaustive pattern match. The following patterns are not covered: `{}`. To ensure a complete match, add these patterns or cover them with a wildcard (`_`) or an identifier.", constructors)
                }
                ExhaustiveCheckError::DeadCode {
                    cause,
                    dead_pattern,
                } => {
                    write!(f, "Error: Dead code detected. The pattern `{}` is unreachable due to the existence of the pattern `{}` prior to it", dead_pattern, cause)
                }
            }
        }
    }

    pub fn check_exhaustive(
        patterns: &[ArmPattern],
        no_arg_constructors: &[&str],
        with_arg_constructors: &[&str],
    ) -> ExhaustiveCheckResult {
        let mut constructor_map: HashMap<String, Vec<ArmPattern>> = HashMap::new();
        let mut found_with_arg = HashMap::new();
        let mut found_no_arg = HashMap::new();
        let mut detected_wild_card_or_identifier = vec![];

        for constructor in with_arg_constructors {
            found_with_arg.insert(constructor.to_string(), false);
        }
        for constructor in no_arg_constructors {
            found_no_arg.insert(constructor.to_string(), false);
        }

        for pattern in patterns {
            dbg!(pattern.clone(), detected_wild_card_or_identifier.clone());
            if !detected_wild_card_or_identifier.is_empty() {
                dbg!("hre??");
                return ExhaustiveCheckResult::dead_code(
                    detected_wild_card_or_identifier
                        .last()
                        .unwrap_or(&ArmPattern::WildCard),
                    pattern,
                );
            }
            match pattern {
                ArmPattern::Constructor(ctor_name, arm_patterns) => {
                    if with_arg_constructors.contains(&ctor_name.as_str()) {
                        constructor_map
                            .entry(ctor_name.clone())
                            .or_insert_with(Vec::new)
                            .extend(arm_patterns.clone());
                        found_with_arg.insert(ctor_name.clone(), true);
                    } else if no_arg_constructors.contains(&ctor_name.as_str()) {
                        found_no_arg.insert(ctor_name.clone(), true);
                    }
                }
                ArmPattern::As(_, inner_pattern) => {
                    if let ArmPattern::Constructor(ctor_name, arm_patterns) = &**inner_pattern {
                        if with_arg_constructors.contains(&ctor_name.as_str()) {
                            constructor_map
                                .entry(ctor_name.clone())
                                .or_insert_with(Vec::new)
                                .extend(arm_patterns.clone());
                            found_with_arg.insert(ctor_name.clone(), true);
                        } else if no_arg_constructors.contains(&ctor_name.as_str()) {
                            found_no_arg.insert(ctor_name.clone(), true);
                        }
                    }
                }
                ArmPattern::WildCard => {
                    detected_wild_card_or_identifier.push(ArmPattern::WildCard);
                }

                arm_pattern if arm_pattern.is_literal_identifier() => {
                    detected_wild_card_or_identifier.push(arm_pattern.clone());
                }
                _ => {}
            }
        }

        let all_with_arg_covered = found_with_arg.values().all(|&v| v);
        let all_no_arg_covered = found_no_arg.values().all(|&v| v);

        if !all_with_arg_covered || !all_no_arg_covered {
            if detected_wild_card_or_identifier.is_empty() {
                let mut missing_with_arg: Vec<_> = found_with_arg
                    .iter()
                    .filter(|(_, &v)| !v)
                    .map(|(k, _)| k.clone())
                    .collect();
                let missing_no_arg: Vec<_> = found_no_arg
                    .iter()
                    .filter(|(_, &v)| !v)
                    .map(|(k, _)| k.clone())
                    .collect();

                missing_with_arg.extend(missing_no_arg.clone());

                return ExhaustiveCheckResult::missing_constructors(missing_with_arg);
            }
        }

        ExhaustiveCheckResult::succeed(ConstructorPatterns(constructor_map))
    }
}

#[cfg(test)]
mod pattern_match_exhaustive_tests {
    use crate::{compile, Expr};

    #[test]
    fn test_option_pattern_match1() {
        let expr = r#"
        let x = some("afsal");
        match x {
            some(a) => a,
            none => "none"
        }
        "#;

        let expr = Expr::from_text(expr).unwrap();

        let result = compile(&expr, &vec![]);
        assert!(result.is_ok())
    }

    #[test]
    fn test_option_pattern_match2() {
        let expr = r#"
        let x = some("afsal");
        match x {
            none => "none",
            some(a) => a
        }
        "#;

        let expr = Expr::from_text(expr).unwrap();

        let result = compile(&expr, &vec![]);
        assert!(result.is_ok())
    }

    #[test]
    fn test_option_pattern_match_wild_card1() {
        let expr = r#"
        let x = some("afsal");
        match x {
            some(_) => a,
            none => "none"
        }
        "#;

        let expr = Expr::from_text(expr).unwrap();
        let result = compile(&expr, &vec![]);
        assert!(result.is_ok())
    }
    #[test]
    fn test_option_pattern_match_wild_card2() {
        let expr = r#"
        let x = some("afsal");
        match x {
            none => "none",
            some(_) => a

        }
        "#;

        let expr = Expr::from_text(expr).unwrap();
        let result = compile(&expr, &vec![]);
        assert!(result.is_ok())
    }

    #[test]
    fn test_option_pattern_match_wild_card3() {
        let expr = r#"
        let x = some("afsal");
        match x {
            some(a) => a,
            _ => "none"
        }
        "#;

        let expr = Expr::from_text(expr).unwrap();
        let result = compile(&expr, &vec![]);
        assert!(result.is_ok())
    }

    #[test]
    fn test_option_pattern_match_wild_card4() {
        let expr = r#"
        let x = some("afsal");
        match x {
            none => "none",
            _ => "none"
        }
        "#;

        let expr = Expr::from_text(expr).unwrap();
        let result = compile(&expr, &vec![]);
        assert!(result.is_ok())
    }

    #[test]
    fn test_option_pattern_match_wild_card5() {
        let expr = r#"
        let x = some("afsal");
        match x {
            some(_) => a,
            _ => "none"
        }
        "#;

        let expr = Expr::from_text(expr).unwrap();
        let result = compile(&expr, &vec![]);
        assert!(result.is_ok())
    }

    #[test]
    fn test_option_pattern_match_wild_card_invalid1() {
        let expr = r#"
        let x = some("afsal");
        match x {
            _ => "none",
            some(_) => a
        }
        "#;

        let expr = Expr::from_text(expr).unwrap();
        let result = compile(&expr, &vec![]).unwrap_err();

        assert_eq!(result, "Error: Dead code detected. The pattern `some(_)` is unreachable due to the existence of the pattern `_` prior to it")
    }

    #[test]
    fn test_option_pattern_match_wild_card_invalid2() {
        let expr = r#"
        let x = some("afsal");
        match x {
            _ => "none",
            none => "a"
        }
        "#;

        let expr = Expr::from_text(expr).unwrap();
        let result = compile(&expr, &vec![]).unwrap_err();
        assert_eq!(result, "Error: Dead code detected. The pattern `none` is unreachable due to the existence of the pattern `_` prior to it")
    }

    #[test]
    fn test_option_pattern_match_identifier_invalid1() {
        let expr = r#"
        let x = some("afsal");
        match x {
            something => "none",
            some(_) => a
        }
        "#;

        let expr = Expr::from_text(expr).unwrap();
        let result = compile(&expr, &vec![]).unwrap_err();
        assert_eq!(result, "Error: Dead code detected. The pattern `some(_)` is unreachable due to the existence of the pattern `something` prior to it")
    }

    #[test]
    fn test_option_pattern_match_identifier_invalid2() {
        let expr = r#"
        let x = some("afsal");
        match x {
            something => "none",
            none => "a"
        }
        "#;

        let expr = Expr::from_text(expr).unwrap();
        let result = compile(&expr, &vec![]).unwrap_err();
        assert_eq!(result, "Error: Dead code detected. The pattern `none` is unreachable due to the existence of the pattern `something` prior to it")
    }

    #[test]
    fn test_option_none_absent() {
        let expr = r#"
        let x = some("afsal");
        match x {
            some(a) => a
        }
        "#;

        let expr = Expr::from_text(expr).unwrap();
        let result = compile(&expr, &vec![]).unwrap_err();

        assert_eq!(result, "Error: Non-exhaustive pattern match. The following patterns are not covered: `none`. To ensure a complete match, add these patterns or cover them with a wildcard (`_`) or an identifier.")
    }

    #[test]
    fn test_option_some_absent() {
        let expr = r#"
        let x = some("afsal");
        match x {
           none => "none"
        }
        "#;

        let expr = Expr::from_text(expr).unwrap();
        let result = compile(&expr, &vec![]).unwrap_err();

        assert_eq!(result, "Error: Non-exhaustive pattern match. The following patterns are not covered: `some`. To ensure a complete match, add these patterns or cover them with a wildcard (`_`) or an identifier.")
    }
}
