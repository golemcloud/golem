use std::collections::VecDeque;
use crate::Expr;


// When checking exhaustive pattern match, there is no need to ensure
// if the pattern aligns with conditions because those checks are done
// as part of previous phases of compilation, and all we need to worry about
// is whether the arms in the pattern match is exhaustive.
pub fn check_exhaustive_pattern_match(expr: &mut Expr) -> Result<(), String> {
    let mut queue = VecDeque::new();
    queue.push_back(expr);

    while let Some(expr) = queue.pop_back() {
        match expr {
            Expr::PatternMatch(_, patterns, _) => {
                let match_arm = patterns.iter().map(|p| p.arm_pattern.clone()).collect::<Vec<_>>();
                internal::check_exhaustive_pattern_match(&match_arm)?;
            }

            expr => expr.visit_children_mut_bottom_up(&mut queue)
        }
    }

    Ok(())
}

mod internal {
    use std::collections::HashMap;
    use crate::{ArmPattern, MatchArm};

    pub fn check_exhaustive_pattern_match(arms: &[ArmPattern]) -> Result<(), String> {
        let optional = check_exhaustivity(
            arms,
            &["some"],
            &["none"],
        ).value()?;

        let result = check_exhaustivity(
            arms,
            &["ok"],
            &["err"],
        ).value()?;

        let constructor_patterns = optional.or(result);

        for (_, patterns) in constructor_patterns.value() {
             check_exhaustive_pattern_match(patterns)?;
        }

        Ok(())
    }

    #[derive(Clone, Debug)]
    struct ConstructorPatterns(HashMap<String, Vec<ArmPattern>>);

    impl ConstructorPatterns {

        pub fn value(&self) -> &HashMap<String, Vec<ArmPattern>> {
            &self.0
        }

        pub fn or(self, other: ConstructorPatterns) -> ConstructorPatterns {
            if self.0.keys().len() == 0 {
                return other;
            }
            self
        }

        pub fn non_empty(&self) -> bool {
            !self.0.is_empty()
        }
    }

    struct ExhaustiveCheckResult(pub Result<ConstructorPatterns, String>);

    impl ExhaustiveCheckResult {

        pub fn value(&self) -> Result<ConstructorPatterns, String> {
            self.0.clone()
        }

        pub fn fail(message: &str) -> Self {
            ExhaustiveCheckResult(Err(message.to_string()))
        }

        pub fn succeed(constructor_patterns: ConstructorPatterns) -> Self {
            ExhaustiveCheckResult(Ok(constructor_patterns))
        }


        pub fn is_valid(&self) -> bool {
            match &self.0 {
                Ok(_) => true,
                Err(_) => false,
            }
        }

        pub fn is_invalid(&self) -> bool {
            !self.is_valid()
        }
    }

    pub fn check_exhaustivity(
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

        // Check if all necessary constructors are covered
        let all_with_arg_covered = found_with_arg.values().all(|&v| v);
        let all_no_arg_covered = found_no_arg.values().all(|&v| v);

        // If both with-arg and no-arg constructors are absent, ensure a wildcard or literal is present
        if !all_with_arg_covered || !all_no_arg_covered {
            if !has_wildcard_or_literal {
                let missing_with_arg: Vec<_> = found_with_arg
                    .iter()
                    .filter(|(_, &v)| !v)
                    .map(|(k, _)| k.clone())
                    .collect();
                let missing_no_arg: Vec<_> = found_no_arg
                    .iter()
                    .filter(|(_, &v)| !v)
                    .map(|(k, _)| k.clone())
                    .collect();

                return ExhaustiveCheckResult::fail(format!(
                    "Missing constructors: {:?}, {:?}",
                    missing_with_arg, missing_no_arg
                ).as_str());
            }
        }

        ExhaustiveCheckResult::succeed(ConstructorPatterns(constructor_map))
    }
}

