// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::{ArmPattern, Expr, ExprVisitor, FunctionTypeRegistry};

// When checking exhaustive pattern match, there is no need to ensure
// if the pattern aligns with conditions because those checks are done
// as part of previous phases of compilation. All we need to worry about
// is whether the arms in the pattern match is exhaustive.
pub fn check_exhaustive_pattern_match(
    expr: &mut Expr,
    function_type_registry: &FunctionTypeRegistry,
) -> Result<(), ExhaustivePatternMatchError> {
    let mut visitor = ExprVisitor::bottom_up(expr);

    while let Some(expr) = visitor.pop_back() {
        if let Expr::PatternMatch { match_arms, .. } = expr {
            let match_arm = match_arms
                .iter()
                .map(|p| p.arm_pattern.clone())
                .collect::<Vec<_>>();
            internal::check_exhaustive_pattern_match(expr, &match_arm, function_type_registry)?;
        }
    }

    Ok(())
}

#[derive(Debug, Clone)]
pub enum ExhaustivePatternMatchError {
    MissingConstructors {
        predicate: Expr,
        missing_constructors: Vec<String>,
    },
    DeadCode {
        predicate: Expr,
        cause: ArmPattern,
        dead_pattern: ArmPattern,
    },
}

mod internal {
    use crate::type_checker::exhaustive_pattern_match::ExhaustivePatternMatchError;
    use crate::{ArmPattern, Expr, FunctionTypeRegistry};
    use golem_wasm_ast::analysis::TypeVariant;
    use std::collections::HashMap;

    use std::ops::Deref;

    pub(crate) fn check_exhaustive_pattern_match(
        predicate: &Expr,
        arms: &[ArmPattern],
        function_registry: &FunctionTypeRegistry,
    ) -> Result<(), ExhaustivePatternMatchError> {
        let mut exhaustive_check_result =
            check_exhaustive(predicate, arms, ConstructorDetail::option());

        let variants = function_registry.get_variants();

        let mut constructor_details = vec![];

        for variant in variants {
            let detail = ConstructorDetail::from_variant(variant);
            constructor_details.push(detail);
        }

        constructor_details.push(ConstructorDetail::option());
        constructor_details.push(ConstructorDetail::result());

        for detail in constructor_details {
            exhaustive_check_result =
                exhaustive_check_result.unwrap_or_run_with(predicate, arms, detail);
        }

        let inner_constructors = exhaustive_check_result.value()?;

        for (field, patterns) in inner_constructors.inner() {
            check_exhaustive_pattern_match(predicate, patterns, function_registry).map_err(
                |e| match e {
                    ExhaustivePatternMatchError::MissingConstructors {
                        missing_constructors,
                        ..
                    } => {
                        let mut new_missing_constructors = vec![];
                        missing_constructors.iter().for_each(|missing_constructor| {
                            new_missing_constructors
                                .push(format!("{}({})", field, missing_constructor));
                        });
                        ExhaustivePatternMatchError::MissingConstructors {
                            predicate: predicate.clone(),
                            missing_constructors: new_missing_constructors,
                        }
                    }
                    other_errors => other_errors,
                },
            )?;
        }

        Ok(())
    }

    #[derive(Clone, Debug)]
    pub struct ConstructorPatterns(HashMap<String, Vec<ArmPattern>>);

    impl ConstructorPatterns {
        pub fn inner(&self) -> &HashMap<String, Vec<ArmPattern>> {
            &self.0
        }

        fn is_empty(&self) -> bool {
            self.0.is_empty()
        }
    }

    #[derive(Debug, Clone)]
    pub(crate) struct ExhaustiveCheckResult(
        pub(crate) Result<ConstructorPatterns, ExhaustivePatternMatchError>,
    );

    impl ExhaustiveCheckResult {
        fn unwrap_or_run_with(
            &self,
            predicate: &Expr,
            patterns: &[ArmPattern],
            constructor_details: ConstructorDetail,
        ) -> ExhaustiveCheckResult {
            match self {
                ExhaustiveCheckResult(Ok(result)) if result.is_empty() => {
                    check_exhaustive(predicate, patterns, constructor_details)
                }
                ExhaustiveCheckResult(Ok(_)) => self.clone(),
                ExhaustiveCheckResult(Err(e)) => ExhaustiveCheckResult(Err(e.clone())),
            }
        }

        fn value(&self) -> Result<ConstructorPatterns, ExhaustivePatternMatchError> {
            self.0.clone()
        }

        fn missing_constructors(predicate: Expr, missing_constructors: Vec<String>) -> Self {
            ExhaustiveCheckResult(Err(ExhaustivePatternMatchError::MissingConstructors {
                predicate,
                missing_constructors,
            }))
        }

        fn dead_code(predicate: &Expr, cause: &ArmPattern, dead_pattern: &ArmPattern) -> Self {
            ExhaustiveCheckResult(Err(ExhaustivePatternMatchError::DeadCode {
                predicate: predicate.clone(),
                cause: cause.clone(),
                dead_pattern: dead_pattern.clone(),
            }))
        }

        fn succeed(constructor_patterns: ConstructorPatterns) -> Self {
            ExhaustiveCheckResult(Ok(constructor_patterns))
        }
    }

    fn check_exhaustive(
        predicate: &Expr,
        patterns: &[ArmPattern],
        pattern_mach_args: ConstructorDetail,
    ) -> ExhaustiveCheckResult {
        let with_arg_constructors = pattern_mach_args.with_arg_constructors;
        let no_arg_constructors = pattern_mach_args.no_arg_constructors;

        let mut constructors_with_arg: ConstructorsWithArgTracker =
            ConstructorsWithArgTracker::new();
        let mut constructors_with_no_arg: NoArgConstructorsTracker =
            NoArgConstructorsTracker::new();
        let mut detected_wild_card_or_identifier: Vec<ArmPattern> = vec![];
        let mut constructor_map_result: HashMap<String, Vec<ArmPattern>> = HashMap::new();

        constructors_with_arg.initialise(with_arg_constructors.clone());
        constructors_with_no_arg.initialise(no_arg_constructors.clone());

        for pattern in patterns {
            if !detected_wild_card_or_identifier.is_empty() {
                return ExhaustiveCheckResult::dead_code(
                    predicate,
                    detected_wild_card_or_identifier
                        .last()
                        .unwrap_or(&ArmPattern::WildCard),
                    pattern,
                );
            }
            match pattern {
                ArmPattern::Constructor(ctor_name, arm_patterns) => {
                    if with_arg_constructors.contains(ctor_name) {
                        constructor_map_result
                            .entry(ctor_name.clone())
                            .or_default()
                            .extend(arm_patterns.clone());
                        constructors_with_arg.register(ctor_name);
                    } else if no_arg_constructors.contains(ctor_name) {
                        constructors_with_no_arg.register(ctor_name);
                    }
                }
                ArmPattern::As(_, inner_pattern) => {
                    if let ArmPattern::Constructor(ctor_name, arm_patterns) = &**inner_pattern {
                        if with_arg_constructors.contains(ctor_name) {
                            constructor_map_result
                                .entry(ctor_name.clone())
                                .or_default()
                                .extend(arm_patterns.clone());
                            constructors_with_arg.register(ctor_name);
                        } else if no_arg_constructors.contains(ctor_name) {
                            constructors_with_no_arg.register(ctor_name);
                        }
                    }
                }
                arm_pattern @ ArmPattern::Literal(expr) => {
                    if let Expr::Call {
                        call_type, args, ..
                    } = expr.deref()
                    {
                        let ctor_name = call_type.to_string();
                        let arm_patterns = args
                            .iter()
                            .map(|arg| ArmPattern::Literal(Box::new(arg.clone())))
                            .collect::<Vec<_>>();
                        if with_arg_constructors.contains(&ctor_name) {
                            constructor_map_result
                                .entry(ctor_name.clone())
                                .or_default()
                                .extend(arm_patterns);
                            constructors_with_arg.register(ctor_name.as_str());
                        } else if no_arg_constructors.contains(&ctor_name) {
                            constructors_with_no_arg.register(ctor_name.as_str());
                        }
                    } else if arm_pattern.is_literal_identifier() {
                        detected_wild_card_or_identifier.push(arm_pattern.clone());
                    }
                }
                ArmPattern::WildCard => {
                    detected_wild_card_or_identifier.push(ArmPattern::WildCard);
                }

                _ => {}
            }
        }

        if constructors_with_arg.registered_any() || constructors_with_no_arg.registered_any() {
            let all_with_arg_covered = constructors_with_arg.registered_all();
            let all_no_arg_covered = constructors_with_no_arg.registered_all();

            if (!all_with_arg_covered || !all_no_arg_covered)
                && detected_wild_card_or_identifier.is_empty()
            {
                let mut missing_constructors: Vec<_> =
                    constructors_with_arg.unregistered_constructors();
                let missing_no_arg_constructors: Vec<_> =
                    constructors_with_no_arg.unregistered_constructors();

                missing_constructors.extend(missing_no_arg_constructors.clone());

                return ExhaustiveCheckResult::missing_constructors(
                    predicate.clone(),
                    missing_constructors,
                );
            }
        }

        // Mainly to handle the scenario of `match x { some(some(x)) => 1, _ => 0 }`. Here the constructor map
        // is "some" -> vec[some(x)]. If we run another exhaustive check (recursively) on the inner pattern which is
        // "some(x)", which becomes non-exhaustive unless we add the `wild-card` or `identifier` pattern into the pattern list before this recursion.
        // In short, outer wild-pattern and identifier patterns have to be appended to every inner pattern before recursively running
        // the exhaustive check. This needs to be done only constructor_map has some elements tracked by this time!
        if !constructor_map_result.is_empty() {
            if !detected_wild_card_or_identifier.is_empty() {
                constructor_map_result.values_mut().for_each(|patterns| {
                    if !patterns.iter().any(|arm_pattern| {
                        arm_pattern.is_literal_identifier() || arm_pattern.is_wildcard()
                    }) {
                        patterns.extend(detected_wild_card_or_identifier.clone());
                    }
                });
            }

            return ExhaustiveCheckResult::succeed(ConstructorPatterns(constructor_map_result));
        }

        ExhaustiveCheckResult::succeed(ConstructorPatterns(constructor_map_result))
    }

    struct ConstructorsWithArgTracker {
        status: HashMap<String, bool>,
    }

    impl ConstructorsWithArgTracker {
        fn new() -> Self {
            ConstructorsWithArgTracker {
                status: HashMap::new(),
            }
        }

        fn initialise(&mut self, with_arg_constructors: Vec<String>) {
            for constructor in with_arg_constructors {
                self.status.insert(constructor.to_string(), false);
            }
        }

        fn register(&mut self, constructor: &str) {
            self.status.insert(constructor.to_string(), true);
        }

        fn registered_any(&self) -> bool {
            self.status.values().any(|&v| v)
        }

        fn registered_all(&self) -> bool {
            self.status.values().all(|&v| v)
        }

        fn unregistered_constructors(&self) -> Vec<String> {
            get_false_entries(&self.status)
        }
    }

    struct NoArgConstructorsTracker {
        status: HashMap<String, bool>,
    }

    impl NoArgConstructorsTracker {
        fn new() -> Self {
            NoArgConstructorsTracker {
                status: HashMap::new(),
            }
        }

        fn initialise(&mut self, no_arg_constructors: Vec<String>) {
            for constructor in no_arg_constructors {
                self.status.insert(constructor.to_string(), false);
            }
        }

        fn register(&mut self, constructor: &str) {
            self.status.insert(constructor.to_string(), true);
        }

        fn registered_any(&self) -> bool {
            self.status.values().any(|&v| v)
        }

        fn registered_all(&self) -> bool {
            self.status.values().all(|&v| v)
        }

        fn unregistered_constructors(&self) -> Vec<String> {
            get_false_entries(&self.status)
        }
    }

    fn get_false_entries(map: &HashMap<String, bool>) -> Vec<String> {
        map.iter()
            .filter(|(_, &v)| !v)
            .map(|(k, _)| k.clone())
            .collect()
    }

    #[derive(Clone, Debug)]
    struct ConstructorDetail {
        no_arg_constructors: Vec<String>,
        with_arg_constructors: Vec<String>,
    }

    impl ConstructorDetail {
        fn from_variant(variant: TypeVariant) -> ConstructorDetail {
            let cases = variant.cases;

            let (no_arg_constructors, with_arg_constructors): (Vec<_>, Vec<_>) =
                cases.into_iter().partition(|c| c.typ.is_none());

            ConstructorDetail {
                no_arg_constructors: no_arg_constructors.iter().map(|c| c.name.clone()).collect(),
                with_arg_constructors: with_arg_constructors
                    .iter()
                    .map(|c| c.name.clone())
                    .collect(),
            }
        }

        fn option() -> Self {
            ConstructorDetail {
                no_arg_constructors: vec!["none".to_string()],
                with_arg_constructors: vec!["some".to_string()],
            }
        }

        fn result() -> Self {
            ConstructorDetail {
                no_arg_constructors: vec![],
                with_arg_constructors: vec!["ok".to_string(), "err".to_string()],
            }
        }
    }
}

#[cfg(test)]
mod pattern_match_exhaustive_tests {
    use crate::type_checker::exhaustive_pattern_match::pattern_match_exhaustive_tests::internal::strip_spaces;
    use crate::{Expr, RibCompiler};
    use test_r::test;

    #[test]
    fn test_option_pattern_match1() {
        let expr = r#"
        let x = some("foo");
        match x {
            some(a) => a,
            none => "none"
        }
        "#;

        let expr = Expr::from_text(expr).unwrap();

        let compiler = RibCompiler::default();
        let result = compiler.compile(expr);
        assert!(result.is_ok())
    }

    #[test]
    fn test_option_pattern_match2() {
        let expr = r#"
        let x = some("foo");
        match x {
            none => "none",
            some(a) => a
        }
        "#;

        let expr = Expr::from_text(expr).unwrap();

        let compiler = RibCompiler::default();
        let result = compiler.compile(expr);
        assert!(result.is_ok())
    }

    #[test]
    fn test_option_pattern_match_wild_card1() {
        let expr = r#"
        let x = some("foo");
        match x {
            some(_) => a,
            none => "none"
        }
        "#;

        let expr = Expr::from_text(expr).unwrap();
        let compiler = RibCompiler::default();
        let result = compiler.compile(expr);
        assert!(result.is_ok())
    }
    #[test]
    fn test_option_pattern_match_wild_card2() {
        let expr = r#"
        let x = some("foo");
        match x {
            none => "none",
            some(_) => a

        }
        "#;

        let expr = Expr::from_text(expr).unwrap();
        let compiler = RibCompiler::default();
        let result = compiler.compile(expr);
        assert!(result.is_ok())
    }

    #[test]
    fn test_option_pattern_match_wild_card3() {
        let expr = r#"
        let x = some("foo");
        match x {
            some(a) => a,
            _ => "none"
        }
        "#;

        let expr = Expr::from_text(expr).unwrap();
        let compiler = RibCompiler::default();
        let result = compiler.compile(expr);
        assert!(result.is_ok())
    }

    #[test]
    fn test_option_pattern_match_wild_card4() {
        let expr = r#"
        let x = some("foo");
        match x {
            none => "none",
            _ => "none"
        }
        "#;

        let expr = Expr::from_text(expr).unwrap();
        let compiler = RibCompiler::default();
        let result = compiler.compile(expr);
        assert!(result.is_ok())
    }

    #[test]
    fn test_option_pattern_match_wild_card5() {
        let expr = r#"
        let x = some("foo");
        match x {
            some(_) => a,
            _ => "none"
        }
        "#;

        let expr = Expr::from_text(expr).unwrap();
        let compiler = RibCompiler::default();
        let result = compiler.compile(expr);
        assert!(result.is_ok())
    }

    #[test]
    fn test_option_pattern_match_wild_card_invalid1() {
        let expr = r#"
        let x = some("foo");
        match x {
            _ => "none",
            some(_) => a
        }
        "#;

        let expr = Expr::from_text(expr).unwrap();

        let compiler = RibCompiler::default();
        let error_msg = compiler.compile(expr).unwrap_err().to_string();

        let expected = r#"
        error in the following rib found at line 3, column 9
        `match x {  _ => "none", some(_) => a } `
        cause: dead code detected, pattern `some(_)` is unreachable due to the existence of the pattern `_` prior to it
        help: to ensure a complete match, add missing patterns or use wildcard (`_`)
        "#;

        assert_eq!(error_msg, strip_spaces(expected))
    }

    #[test]
    fn test_option_pattern_match_wild_card_invalid2() {
        let expr = r#"
        let x = some("foo");
        match x {
            _ => "none",
            none => "a"
        }
        "#;

        let expr = Expr::from_text(expr).unwrap();

        let compiler = RibCompiler::default();
        let error_msg = compiler.compile(expr).unwrap_err().to_string();

        let expected = r#"
        error in the following rib found at line 3, column 9
        `match x {  _ => "none", none => "a" } `
        cause: dead code detected, pattern `none` is unreachable due to the existence of the pattern `_` prior to it
        help: to ensure a complete match, add missing patterns or use wildcard (`_`)
        "#;

        assert_eq!(error_msg, strip_spaces(expected))
    }

    #[test]
    fn test_option_pattern_match_identifier_invalid1() {
        let expr = r#"
        let x = some("foo");
        match x {
            something => "none",
            some(_) => a
        }
        "#;

        let expr = Expr::from_text(expr).unwrap();

        let compiler = RibCompiler::default();
        let error_msg = compiler.compile(expr).unwrap_err().to_string();

        let expected = r#"
        error in the following rib found at line 3, column 9
        `match x {  something => "none", some(_) => a } `
        cause: dead code detected, pattern `some(_)` is unreachable due to the existence of the pattern `something` prior to it
        help: to ensure a complete match, add missing patterns or use wildcard (`_`)
        "#;

        assert_eq!(error_msg, strip_spaces(expected))
    }

    #[test]
    fn test_option_pattern_match_identifier_invalid2() {
        let expr = r#"
        let x = some("foo");
        match x {
            something => "none",
            none => "a"
        }
        "#;
        let expr = Expr::from_text(expr).unwrap();

        let compiler = RibCompiler::default();
        let error_msg = compiler.compile(expr).unwrap_err().to_string();

        let expected = r#"
        error in the following rib found at line 3, column 9
        `match x {  something => "none", none => "a" } `
        cause: dead code detected, pattern `none` is unreachable due to the existence of the pattern `something` prior to it
        help: to ensure a complete match, add missing patterns or use wildcard (`_`)
        "#;

        assert_eq!(error_msg, strip_spaces(expected))
    }

    #[test]
    fn test_option_none_absent() {
        let expr = r#"
        let x = some("foo");
        match x {
            some(a) => a
        }
        "#;

        let expr = Expr::from_text(expr).unwrap();

        let compiler = RibCompiler::default();
        let error_msg = compiler.compile(expr).unwrap_err().to_string();

        let expected = r#"
        error in the following rib found at line 3, column 9
        `match x {  some(a) => a } `
        cause: non-exhaustive pattern match: the following patterns are not covered: `none`
        help: to ensure a complete match, add missing patterns or use wildcard (`_`)
        "#;

        assert_eq!(error_msg, strip_spaces(expected));
    }

    #[test]
    fn test_option_some_absent() {
        let expr = r#"
        let x = some("foo");
        match x {
           none => "none"
        }
        "#;

        let expr = Expr::from_text(expr).unwrap();

        let compiler = RibCompiler::default();
        let error_msg = compiler.compile(expr).unwrap_err().to_string();

        let expected = r#"
        error in the following rib found at line 3, column 9
        `match x {  none => "none" } `
        cause: non-exhaustive pattern match: the following patterns are not covered: `some`
        help: to ensure a complete match, add missing patterns or use wildcard (`_`)
        "#;

        assert_eq!(error_msg, strip_spaces(expected));
    }

    #[test]
    fn test_option_nested_invalid1() {
        let expr = r#"
        let x = some(some("foo"));
        match x {
            some(some(a)) => a,
            none => "bar"
        }
        "#;

        let expr = Expr::from_text(expr).unwrap();

        let compiler = RibCompiler::default();
        let error_msg = compiler.compile(expr).unwrap_err().to_string();

        let expected = r#"
        error in the following rib found at line 3, column 9
        `match x {  some(some(a)) => a, none => "bar" } `
        cause: non-exhaustive pattern match: the following patterns are not covered: `some(none)`
        help: to ensure a complete match, add missing patterns or use wildcard (`_`)
        "#;

        assert_eq!(error_msg, strip_spaces(expected));
    }

    #[test]
    fn test_result_pattern_match1() {
        let expr = r#"
        let x: result<string, string> = ok("foo");
        match x {
            ok(a) => a,
            err(msg) =>  msg
        }
        "#;

        let expr = Expr::from_text(expr).unwrap();

        let compiler = RibCompiler::default();
        let result = compiler.compile(expr);
        assert!(result.is_ok())
    }

    #[test]
    fn test_result_pattern_match2() {
        let expr = r#"
        let x: result<string, string> = ok("foo");
        match x {
            err(a) => a,
            ok(a) => a
        }
        "#;

        let expr = Expr::from_text(expr).unwrap();

        let compiler = RibCompiler::default();
        let result = compiler.compile(expr);
        assert!(result.is_ok())
    }

    #[test]
    fn test_result_pattern_match_wild_card1() {
        let expr = r#"
        let x = ok("foo");
        match x {
            err(_) => "error",
            ok(msg) => msg
        }
        "#;

        let expr = Expr::from_text(expr).unwrap();
        let compiler = RibCompiler::default();
        let result = compiler.compile(expr);
        assert!(result.is_ok())
    }
    #[test]
    fn test_result_pattern_match_wild_card2() {
        let expr = r#"
        let x: result<string, string> = ok("foo");
        match x {
            err(msg) => msg,
            ok(_) => a

        }
        "#;

        let expr = Expr::from_text(expr).unwrap();
        let compiler = RibCompiler::default();
        let result = compiler.compile(expr);
        assert!(result.is_ok())
    }

    #[test]
    fn test_result_pattern_match_wild_card3() {
        let expr = r#"
        let x = ok("foo");
        match x {
            ok(a) => a,
            _ => "none"
        }
        "#;

        let expr = Expr::from_text(expr).unwrap();
        let compiler = RibCompiler::default();
        let result = compiler.compile(expr);
        assert!(result.is_ok())
    }

    #[test]
    fn test_result_pattern_match_wild_card4() {
        let expr = r#"
        let x = err("foo");
        match x {
            err(msg) => "none",
            _ => "none"
        }
        "#;

        let expr = Expr::from_text(expr).unwrap();
        let compiler = RibCompiler::default();
        let result = compiler.compile(expr);
        assert!(result.is_ok())
    }

    #[test]
    fn test_result_pattern_match_wild_card5() {
        let expr = r#"
        let x = ok("foo");
        match x {
            ok(_) => a,
            _ => "none"
        }
        "#;

        let expr = Expr::from_text(expr).unwrap();
        let compiler = RibCompiler::default();
        let result = compiler.compile(expr);
        assert!(result.is_ok())
    }

    #[test]
    fn test_result_pattern_match_wild_card_invalid1() {
        let expr = r#"
        let x = ok("foo");
        match x {
            _ => "none",
            ok(_) => a
        }
        "#;

        let expr = Expr::from_text(expr).unwrap();

        let compiler = RibCompiler::default();
        let error_msg = compiler.compile(expr).unwrap_err().to_string();

        let expected = r#"
        error in the following rib found at line 3, column 9
        `match x {  _ => "none", ok(_) => a } `
        cause: dead code detected, pattern `ok(_)` is unreachable due to the existence of the pattern `_` prior to it
        help: to ensure a complete match, add missing patterns or use wildcard (`_`)
        "#;

        assert_eq!(error_msg, strip_spaces(expected));
    }

    #[test]
    fn test_result_pattern_match_wild_card_invalid2() {
        let expr = r#"
        let x = err("foo");
        match x {
            _ => "none",
            err(msg) => "a"
        }
        "#;

        let expr = Expr::from_text(expr).unwrap();

        let compiler = RibCompiler::default();
        let error_msg = compiler.compile(expr).unwrap_err().to_string();

        let expected = r#"
        error in the following rib found at line 3, column 9
        `match x {  _ => "none", err(msg) => "a" } `
        cause: dead code detected, pattern `err(msg)` is unreachable due to the existence of the pattern `_` prior to it
        help: to ensure a complete match, add missing patterns or use wildcard (`_`)
        "#;

        assert_eq!(error_msg, strip_spaces(expected));
    }

    #[test]
    fn test_result_pattern_match_identifier_invalid1() {
        let expr = r#"
        let x = ok("foo");
        match x {
            something => "none",
            ok(_) => "a"
        }
        "#;

        let expr = Expr::from_text(expr).unwrap();

        let compiler = RibCompiler::default();
        let error_msg = compiler.compile(expr).unwrap_err().to_string();

        let expected = r#"
        error in the following rib found at line 3, column 9
        `match x {  something => "none", ok(_) => "a" } `
        cause: dead code detected, pattern `ok(_)` is unreachable due to the existence of the pattern `something` prior to it
        help: to ensure a complete match, add missing patterns or use wildcard (`_`)
        "#;

        assert_eq!(error_msg, strip_spaces(expected));
    }

    #[test]
    fn test_result_pattern_match_identifier_invalid2() {
        let expr = r#"
        let x = err("foo");
        match x {
            something => "none",
            err(msg) => "a"
        }
        "#;

        let expr = Expr::from_text(expr).unwrap();

        let compiler = RibCompiler::default();
        let error_msg = compiler.compile(expr).unwrap_err().to_string();

        let expected = r#"
        error in the following rib found at line 3, column 9
        `match x {  something => "none", err(msg) => "a" } `
        cause: dead code detected, pattern `err(msg)` is unreachable due to the existence of the pattern `something` prior to it
        help: to ensure a complete match, add missing patterns or use wildcard (`_`)
        "#;

        assert_eq!(error_msg, strip_spaces(expected));
    }

    #[test]
    fn test_result_err_absent() {
        let expr = r#"
        let x = ok("foo");
        match x {
            ok(a) => a
        }
        "#;

        let expr = Expr::from_text(expr).unwrap();

        let compiler = RibCompiler::default();
        let error_msg = compiler.compile(expr).unwrap_err().to_string();

        let expected = r#"
        error in the following rib found at line 3, column 9
        `match x {  ok(a) => a } `
        cause: non-exhaustive pattern match: the following patterns are not covered: `err`
        help: to ensure a complete match, add missing patterns or use wildcard (`_`)
        "#;

        assert_eq!(error_msg, strip_spaces(expected));
    }

    #[test]
    fn test_result_ok_absent() {
        // Explicit type annotation is required here otherwise `str` in `err` cannot be inferred
        let expr = r#"
        let x: result<string, string> = ok("foo");
        match x {
           err(str) => str
        }
        "#;

        let expr = Expr::from_text(expr).unwrap();

        let compiler = RibCompiler::default();
        let error_msg = compiler.compile(expr).unwrap_err().to_string();

        let expected = r#"
        error in the following rib found at line 3, column 9
        `match x {  err(str) => str } `
        cause: non-exhaustive pattern match: the following patterns are not covered: `ok`
        help: to ensure a complete match, add missing patterns or use wildcard (`_`)
        "#;

        assert_eq!(error_msg, strip_spaces(expected));
    }

    #[test]
    fn test_result_nested_invalid1() {
        let expr = r#"
        let x = ok(err("foo"));
        match x {
            ok(err(a)) => a,
            err(_) => "bar"
        }
        "#;

        let expr = Expr::from_text(expr).unwrap();

        let compiler = RibCompiler::default();
        let error_msg = compiler.compile(expr).unwrap_err().to_string();

        let expected = r#"
        error in the following rib found at line 3, column 9
        `match x {  ok(err(a)) => a, err(_) => "bar" } `
        cause: non-exhaustive pattern match: the following patterns are not covered: `ok(ok)`
        help: to ensure a complete match, add missing patterns or use wildcard (`_`)
        "#;

        assert_eq!(error_msg, strip_spaces(expected));
    }

    #[test]
    fn test_result_nested_invalid2() {
        let expr = r#"
        let x = ok(ok("foo"));
        match x {
            ok(ok(a)) => a,
            err(_) => "bar"
        }
        "#;

        let expr = Expr::from_text(expr).unwrap();

        let compiler = RibCompiler::default();
        let error_msg = compiler.compile(expr).unwrap_err().to_string();

        let expected = r#"
        error in the following rib found at line 3, column 9
        `match x {  ok(ok(a)) => a, err(_) => "bar" } `
        cause: non-exhaustive pattern match: the following patterns are not covered: `ok(err)`
        help: to ensure a complete match, add missing patterns or use wildcard (`_`)
        "#;

        assert_eq!(error_msg, strip_spaces(expected));
    }

    #[test]
    fn test_result_wild_card_with_nested1() {
        let expr = r#"
        let x = ok(ok("foo"));
        match x {
            ok(ok(a)) => a,
            _ => "bar"
        }
        "#;

        let expr = Expr::from_text(expr).unwrap();
        let compiler = RibCompiler::default();
        let result = compiler.compile(expr);

        assert!(result.is_ok());
    }

    #[test]
    fn test_result_wild_card_with_nested2() {
        let expr = r#"
        let x = err(err("foo"));
        match x {
            err(err(a)) => a,
            _ => "bar"
        }
        "#;

        let expr = Expr::from_text(expr).unwrap();
        let compiler = RibCompiler::default();
        let result = compiler.compile(expr);

        assert!(result.is_ok());
    }

    #[test]
    fn test_option_wild_card_with_nested1() {
        let expr = r#"
        let x = some(some("foo"));
        match x {
            some(some(a)) => a,
            _ => "bar"
        }
        "#;

        let expr = Expr::from_text(expr).unwrap();
        let compiler = RibCompiler::default();
        let result = compiler.compile(expr);

        assert!(result.is_ok());
    }

    mod internal {
        pub(crate) fn strip_spaces(input: &str) -> String {
            let lines = input.lines();

            let first_line = lines
                .clone()
                .find(|line| !line.trim().is_empty())
                .unwrap_or("");
            let margin_width = first_line.chars().take_while(|c| c.is_whitespace()).count();

            let result = lines
                .map(|line| {
                    if line.trim().is_empty() {
                        String::new()
                    } else {
                        line[margin_width..].to_string()
                    }
                })
                .collect::<Vec<String>>()
                .join("\n");

            result.strip_prefix("\n").unwrap_or(&result).to_string()
        }
    }
}
