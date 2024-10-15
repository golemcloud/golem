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

        let inner_constructors = check_result.value()?;

        for (_, patterns) in inner_constructors.value() {
            check_exhaustive_pattern_match(patterns)?;
        }

        Ok(())
    }

    #[derive(Clone, Debug)]
    pub struct ConstructorPatterns(HashMap<String, Vec<ArmPattern>>);

    impl ConstructorPatterns {
        pub fn empty() -> Self {
            ConstructorPatterns(HashMap::new())
        }

        pub fn value(&self) -> &HashMap<String, Vec<ArmPattern>> {
            &self.0
        }

        pub fn or(self, other: ConstructorPatterns) -> ConstructorPatterns {
            if self.0.keys().len() == 0 {
                return other;
            }
            self
        }

        pub fn is_empty(&self) -> bool {
            self.0.keys().len() == 0
        }
    }

    #[derive(Clone)]
    pub struct ExhaustiveCheckResult(pub Result<ConstructorPatterns, ExhaustiveCheckError>);

    #[derive(Clone)]
    pub enum ExhaustiveCheckError {
        MissingConstructors(Vec<String>),
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

        pub fn fail(missing_constructors: Vec<String>) -> Self {
            ExhaustiveCheckResult(Err(ExhaustiveCheckError::MissingConstructors(
                missing_constructors,
            )))
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
        let mut has_wildcard_or_literal = false;

        // Initialize the tracking for all constructors
        for constructor in with_arg_constructors {
            found_with_arg.insert(constructor.to_string(), false);
        }
        for constructor in no_arg_constructors {
            found_no_arg.insert(constructor.to_string(), false);
        }

        for pattern in patterns {
            match pattern {
                // Handle with-argument constructors
                ArmPattern::Constructor(ctor_name, arm_patterns) => {
                    dbg!(ctor_name.clone());
                    dbg!(with_arg_constructors.clone());
                    if with_arg_constructors.contains(&ctor_name.as_str()) {
                        constructor_map
                            .entry(ctor_name.clone())
                            .or_insert_with(Vec::new)
                            .extend(arm_patterns.clone());
                        dbg!(constructor_map.clone());
                        found_with_arg.insert(ctor_name.clone(), true);
                    } else if no_arg_constructors.contains(&ctor_name.as_str()) {
                        found_no_arg.insert(ctor_name.clone(), true);
                    }
                }
                // Handle `As` pattern for with-argument or no-argument constructors
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
                // Check for wildcard or literal presence
                ArmPattern::WildCard | ArmPattern::Literal(_) => {
                    has_wildcard_or_literal = true;
                }
                _ => {} // Ignore other patterns
            }
        }

        dbg!(constructor_map.clone());

        // Check if all necessary constructors are covered
        let all_with_arg_covered = found_with_arg.values().all(|&v| v);
        let all_no_arg_covered = found_no_arg.values().all(|&v| v);

        dbg!(all_no_arg_covered);
        dbg!(all_with_arg_covered);

        // If both with-arg and no-arg constructors are absent, ensure a wildcard or literal is present
        if !all_with_arg_covered || !all_no_arg_covered {
            if !has_wildcard_or_literal {
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

                dbg!(missing_with_arg.clone());

                return ExhaustiveCheckResult::fail(missing_with_arg);
            }
        }

        ExhaustiveCheckResult::succeed(ConstructorPatterns(constructor_map))
    }
}

#[cfg(test)]
mod pattern_match_exhaustive_tests {
    use crate::{compile, Expr};

    #[test]
    fn test_option_pattern_match() {
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
    fn test_option_pattern_match_wild_card() {
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
    fn test_option_pattern_match_inverted() {
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
    fn test_option_pattern_match_wild_card_inverted() {
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
    fn test_option_none_absent() {
        let expr = r#"
        let x = some("afsal");
        match x {
            some(a) => a
        }
        "#;

        let expr = Expr::from_text(expr).unwrap();
        let result = compile(&expr, &vec![]).unwrap_err();

        dbg!(result.clone().to_string());
        assert_eq!(result, "Missing constructors: [\"none\"], []")
    }
}
