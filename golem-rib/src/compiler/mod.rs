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

pub use byte_code::*;
pub use compiler_output::*;
pub use ir::*;
pub use type_with_unit::*;
pub use worker_functions_in_rib::*;

use crate::rib_type_error::RibTypeError;
use crate::{
    ComponentDependencies, ComponentDependencyKey, CustomInstanceSpec, Expr,
    GlobalVariableTypeSpec, InferredExpr, RibInputTypeInfo, RibOutputTypeInfo,
};
use golem_wasm::analysis::{AnalysedExport, TypeEnum, TypeVariant};
use std::error::Error;
use std::fmt::Display;

mod byte_code;
mod compiler_output;
mod desugar;
mod ir;
mod type_with_unit;
mod worker_functions_in_rib;

#[derive(Default)]
pub struct RibCompiler {
    component_dependency: ComponentDependencies,
    global_variable_type_spec: Vec<GlobalVariableTypeSpec>,
    custom_instance_spec: Vec<CustomInstanceSpec>,
}

impl RibCompiler {
    pub fn new(config: RibCompilerConfig) -> RibCompiler {
        let component_dependencies = ComponentDependencies::from_raw(
            config
                .component_dependencies
                .iter()
                .map(|dep| (dep.component_dependency_key.clone(), &dep.component_exports))
                .collect::<Vec<_>>(),
        )
        .unwrap();

        let global_variable_type_spec = config.input_spec;

        RibCompiler {
            component_dependency: component_dependencies,
            global_variable_type_spec,
            custom_instance_spec: config.custom_instance_spec,
        }
    }

    pub fn infer_types(&self, expr: Expr) -> Result<InferredExpr, RibCompilationError> {
        InferredExpr::from_expr(
            expr.clone(),
            &self.component_dependency,
            &self.global_variable_type_spec,
            &self.custom_instance_spec,
        )
        .map_err(|err| {
            let rib_type_error = RibTypeError::from_rib_type_error_internal(err, expr);
            RibCompilationError::RibTypeError(Box::new(rib_type_error))
        })
    }

    pub fn get_custom_instance_names(&self) -> Vec<String> {
        self.custom_instance_spec
            .iter()
            .map(|spec| spec.instance_name.clone())
            .collect::<Vec<_>>()
    }

    // Currently supports only 1 component and hence really only one InstanceType
    pub fn get_component_dependencies(&self) -> ComponentDependencies {
        self.component_dependency.clone()
    }

    pub fn compile(&self, expr: Expr) -> Result<CompilerOutput, RibCompilationError> {
        let inferred_expr = self.infer_types(expr)?;

        let function_calls_identified =
            WorkerFunctionsInRib::from_inferred_expr(&inferred_expr, &self.component_dependency)?;

        // The types that are tagged as global input in the script
        let global_input_type_info = RibInputTypeInfo::from_expr(&inferred_expr)?;
        let output_type_info = RibOutputTypeInfo::from_expr(&inferred_expr)?;

        // allowed_global_variables
        let allowed_global_variables: Vec<String> = self
            .global_variable_type_spec
            .iter()
            .map(|x| x.variable())
            .collect::<Vec<_>>();

        let mut unidentified_global_inputs = vec![];

        if !allowed_global_variables.is_empty() {
            for (name, _) in global_input_type_info.types.iter() {
                if !allowed_global_variables.contains(name) {
                    unidentified_global_inputs.push(name.clone());
                }
            }
        }

        if !unidentified_global_inputs.is_empty() {
            return Err(RibCompilationError::UnsupportedGlobalInput {
                invalid_global_inputs: unidentified_global_inputs,
                valid_global_inputs: allowed_global_variables,
            });
        }

        let byte_code = RibByteCode::from_expr(&inferred_expr)?;

        Ok(CompilerOutput {
            worker_invoke_calls: function_calls_identified,
            byte_code,
            rib_input_type_info: global_input_type_info,
            rib_output_type_info: Some(output_type_info),
        })
    }

    pub fn get_variants(&self) -> Vec<TypeVariant> {
        self.component_dependency.get_variants()
    }

    pub fn get_enums(&self) -> Vec<TypeEnum> {
        self.component_dependency.get_enums()
    }
}

/// Compiler configuration options for Rib.
///
/// # Fields
/// - `component_metadata`: Component metadata that describes the worker functions available.
/// - `global_input_spec`: Defines constraints and types for global input variables.
///   By default, Rib allows any identifier (e.g., `foo`) to be treated as a global variable.
///   A global variable is a variable that is not defined in the Rib script but is expected to be provided
///   by the environment in which the Rib script is executed (e.g., `request`, `env`). Hence it is called `global_input`.
///   This field can restrict global variables to a predefined set. If the field is empty, any identifier
///   can be used as a global variable.
///
///   You can also associate specific types with known global variables using
///   `GlobalVariableTypeSpec`. For example, the path `request.path.*` can be enforced to always
///   be of type `string`. Note that not all global variables require a type specification.
#[derive(Default)]
pub struct RibCompilerConfig {
    component_dependencies: Vec<ComponentDependency>,
    input_spec: Vec<GlobalVariableTypeSpec>,
    custom_instance_spec: Vec<CustomInstanceSpec>,
}

impl RibCompilerConfig {
    pub fn new(
        component_dependencies: Vec<ComponentDependency>,
        input_spec: Vec<GlobalVariableTypeSpec>,
        custom_instance_spec: Vec<CustomInstanceSpec>,
    ) -> RibCompilerConfig {
        RibCompilerConfig {
            component_dependencies,
            input_spec,
            custom_instance_spec,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ComponentDependency {
    component_dependency_key: ComponentDependencyKey,
    component_exports: Vec<AnalysedExport>,
}

impl ComponentDependency {
    pub fn new(
        component_dependency_key: ComponentDependencyKey,
        component_exports: Vec<AnalysedExport>,
    ) -> Self {
        Self {
            component_dependency_key,
            component_exports,
        }
    }
}

pub trait GenerateWorkerName {
    fn generate_worker_name(&self) -> String;
}

pub struct DefaultWorkerNameGenerator;

impl GenerateWorkerName for DefaultWorkerNameGenerator {
    fn generate_worker_name(&self) -> String {
        let uuid = uuid::Uuid::new_v4();
        format!("worker-{uuid}")
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum RibCompilationError {
    // Bytecode generation errors should ideally never occur.
    // They are considered programming errors that indicate some part of type checking
    // or inference needs to be fixed.
    ByteCodeGenerationFail(Box<RibByteCodeGenerationError>),

    // RibTypeError is a type error that occurs during type inference.
    // This is a typical compilation error, such as: expected u32, found str.
    RibTypeError(Box<RibTypeError>),

    // This captures only the syntax parse errors in a Rib script.
    InvalidSyntax(String),

    // This occurs when the Rib script includes global inputs that cannot be
    // fulfilled. For example, if Rib is used from a REPL, the only valid global input will be `env`.
    // If it is used from the Golem API gateway, it is  `request`.
    // If the user specifies a global input such as `foo`
    // (e.g., the compiler will treat `foo` as a global input in a Rib script like `my-worker-function(foo)`),
    // it will fail compilation with this error.
    // Note: the type inference phase will still be happy with this Rib script;
    // we perform this validation as an extra step at the end to allow clients of `golem-rib`
    // to decide what global inputs are valid.
    UnsupportedGlobalInput {
        invalid_global_inputs: Vec<String>,
        valid_global_inputs: Vec<String>,
    },

    // A typical use of static analysis in Rib is to identify all the valid worker functions.
    // If this analysis phase fails, it typically indicates a bug in the Rib compiler.
    RibStaticAnalysisError(String),
}

impl From<RibByteCodeGenerationError> for RibCompilationError {
    fn from(err: RibByteCodeGenerationError) -> Self {
        RibCompilationError::ByteCodeGenerationFail(Box::new(err))
    }
}

impl From<RibTypeError> for RibCompilationError {
    fn from(err: RibTypeError) -> Self {
        RibCompilationError::RibTypeError(Box::new(err))
    }
}

impl Display for RibCompilationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RibCompilationError::RibStaticAnalysisError(msg) => {
                write!(f, "rib static analysis error: {msg}")
            }
            RibCompilationError::RibTypeError(err) => write!(f, "{err}"),
            RibCompilationError::InvalidSyntax(msg) => write!(f, "invalid rib syntax: {msg}"),
            RibCompilationError::UnsupportedGlobalInput {
                invalid_global_inputs,
                valid_global_inputs,
            } => {
                write!(
                    f,
                    "unsupported global input variables: {}. expected: {}",
                    invalid_global_inputs.join(", "),
                    valid_global_inputs.join(", ")
                )
            }
            RibCompilationError::ByteCodeGenerationFail(e) => {
                write!(f, "{e}")
            }
        }
    }
}

impl Error for RibCompilationError {}

#[cfg(test)]
mod compiler_error_tests {
    mod type_mismatch_errors {
        use test_r::test;

        use crate::compiler::compiler_error_tests::test_utils;
        use crate::compiler::compiler_error_tests::test_utils::strip_spaces;
        use crate::{Expr, RibCompiler, RibCompilerConfig};

        #[test]
        async fn test_invalid_pattern_match0() {
            let expr = r#"
          match 1 {
            1 =>  {  foo : "bar"  },
            2 =>  {  foo : 1  }
          }

        "#;

            let expr = Expr::from_text(expr).unwrap();

            let metadata = test_utils::get_metadata();

            let compiler = RibCompiler::new(RibCompilerConfig::new(metadata, vec![], vec![]));
            let error_msg = compiler.compile(expr).unwrap_err().to_string();

            let expected = r#"
            error in the following rib found at line 3, column 28
            `"bar"`
            cause: type mismatch. expected s32, found string
            the expression `"bar"` is inferred as `string` by default
            "#;

            assert_eq!(error_msg, test_utils::strip_spaces(expected));
        }

        #[test]
        async fn test_invalid_pattern_match1() {
            let expr = r#"
          let x = 1;
          match some(x) {
            some(_) => {foo: x},
            none => {foo: "bar"}
          }
        "#;

            let expr = Expr::from_text(expr).unwrap();

            let metadata = test_utils::get_metadata();

            let compiler = RibCompiler::new(RibCompilerConfig::new(metadata, vec![], vec![]));
            let error_msg = compiler.compile(expr).unwrap_err().to_string();

            let expected = r#"
            error in the following rib found at line 2, column 19
            `1`
            cause: type mismatch. expected string, found s32
            the expression `1` is inferred as `s32` by default
            "#;

            assert_eq!(error_msg, test_utils::strip_spaces(expected));
        }

        #[test]
        async fn test_invalid_pattern_match2() {
            let expr = r#"
          let x: option<u64> = some(1);
          match x {
            some(x) => ok(x),
            none    => ok("none")
          }
        "#;

            let expr = Expr::from_text(expr).unwrap();

            let metadata = test_utils::get_metadata();

            let compiler = RibCompiler::new(RibCompilerConfig::new(metadata, vec![], vec![]));
            let error_msg = compiler.compile(expr).unwrap_err().to_string();

            let expected = r#"
            error in the following rib found at line 5, column 27
            `"none"`
            cause: type mismatch. expected u64, found string
            expected type u64 based on expression `x` found at line 4 column 27
            the expression `"none"` is inferred as `string` by default
            "#;

            assert_eq!(error_msg, test_utils::strip_spaces(expected));
        }

        #[test]
        async fn test_invalid_pattern_match3() {
            let expr = r#"
          let x: option<u64> = some(1);
          match x {
            some(x) => ok("none"),
            none    => ok(1)
          }
        "#;

            let expr = Expr::from_text(expr).unwrap();

            let metadata = test_utils::get_metadata();

            let compiler = RibCompiler::new(RibCompilerConfig::new(metadata, vec![], vec![]));
            let error_msg = compiler.compile(expr).unwrap_err().to_string();

            let expected = r#"
            error in the following rib found at line 4, column 27
            `"none"`
            cause: type mismatch. expected s32, found string
            expected type s32 based on expression `1` found at line 5 column 27
            the expression `1` is inferred as `s32` by default
            the expression `"none"` is inferred as `string` by default
            "#;

            assert_eq!(error_msg, test_utils::strip_spaces(expected));
        }

        #[test]
        async fn test_invalid_pattern_match4() {
            let expr = r#"
          let x: s32 = 1;
          let y: u64 = 2;

          match some(1) {
            some(_) => ok(x),
            none    => ok(y)
          }
        "#;

            let expr = Expr::from_text(expr).unwrap();

            let metadata = test_utils::get_metadata();

            let compiler = RibCompiler::new(RibCompilerConfig::new(metadata, vec![], vec![]));
            let error_msg = compiler.compile(expr).unwrap_err().to_string();

            let expected = r#"
            error in the following rib found at line 7, column 27
            `y`
            cause: type mismatch. expected s32, found u64
            expected type s32 based on expression `x` found at line 6 column 27
            the type of `x` is declared as `s32` at line 2 column 11
            the type of `y` is declared as `u64` at line 3 column 11
            "#;

            assert_eq!(error_msg, test_utils::strip_spaces(expected));
        }

        #[test]
        fn test_invalid_function_call0() {
            let expr = r#"
          let result = foo(1);
          result
        "#;

            let expr = Expr::from_text(expr).unwrap();

            let metadata = test_utils::get_metadata();

            let compiler = RibCompiler::new(RibCompilerConfig::new(metadata, vec![], vec![]));
            let error_msg = compiler.compile(expr).unwrap_err().to_string();

            let expected = r#"
            error in the following rib found at line 2, column 28
            `1`
            cause: type mismatch. expected record { a: record { aa: s32, ab: s32, ac: list<s32>, ad: record { ada: s32 }, ae: tuple<s32, string> }, b: u64, c: list<s32>, d: record { da: s32 } }, found s32
            invalid argument to the function `foo`
            "#;

            assert_eq!(error_msg, test_utils::strip_spaces(expected));
        }

        #[test]
        fn test_invalid_function_call1asdasd() {
            let expr = r#"
          let worker = instance("my-worker");
          let result = worker.foo({a: {aa: 1, ab: 2, ac: [1, 2], ad: {ada: 1}, ae: (1, "foo")}, b: "foo", c: [1, 2, 3], d: {da: 4}});
          result
        "#;

            let expr = Expr::from_text(expr).unwrap();

            let metadata = test_utils::get_metadata();

            let compiler = RibCompiler::new(RibCompilerConfig::new(metadata, vec![], vec![]));
            let error_msg = compiler.compile(expr).unwrap_err().to_string();

            let expected = r#"
            error in the following rib found at line 3, column 100
            `"foo"`
            cause: type mismatch. expected u64, found string
            the expression `"foo"` is inferred as `string` by default
            "#;

            assert_eq!(error_msg, test_utils::strip_spaces(expected));
        }

        #[test]
        fn test_invalid_function_call2() {
            let expr = r#"
          let worker = instance("my-worker");
          let result = worker.foo({a: {aa: 1, ab: 2, ac: [1, 2], ad: {ada: 1}, ae: (1, "foo")}, b: 2, c: ["foo", "bar"], d: {da: 4}});
          result
        "#;

            let expr = Expr::from_text(expr).unwrap();

            let metadata = test_utils::get_metadata();

            let compiler = RibCompiler::new(RibCompilerConfig::new(metadata, vec![], vec![]));
            let error_msg = compiler.compile(expr).unwrap_err().to_string();

            let expected = r#"
            error in the following rib found at line 3, column 107
            `"foo"`
            cause: type mismatch. expected s32, found string
            the expression `"foo"` is inferred as `string` by default
            "#;

            assert_eq!(error_msg, test_utils::strip_spaces(expected));
        }

        #[test]
        fn test_invalid_function_call3() {
            let expr = r#"
          let worker = instance();
          let result = worker.foo({a: {aa: 1, ab: 2, ac: [1, 2], ad: {ada: 1}, ae: (1, "foo")}, b: 2, c: [1, 2], d: {da: "foo"}});
          result
        "#;

            let expr = Expr::from_text(expr).unwrap();

            let metadata = test_utils::get_metadata();

            let compiler = RibCompiler::new(RibCompilerConfig::new(metadata, vec![], vec![]));
            let error_msg = compiler.compile(expr).unwrap_err().to_string();

            let expected = r#"
            error in the following rib found at line 3, column 122
            `"foo"`
            cause: type mismatch. expected s32, found string
            the expression `"foo"` is inferred as `string` by default
            "#;

            assert_eq!(error_msg, test_utils::strip_spaces(expected));
        }

        // Here the difference is, the shape itself is different losing the preciseness of the error.
        // The best precise error
        // is type-mismatch, however, here we get an ambiguity error. This can be improved,
        // by not allowing accumulation of conflicting types into Exprs that are part of a function call
        #[test]
        fn test_invalid_function_call4() {
            let expr = r#"
          let result = foo({a: {aa: 1, ab: 2, ac: (1, 2), ad: {ada: 1}, ae: (1, "foo")}, b: 2, c: [1, 2], d: {da: 1}});
          result
        "#;

            let expr = Expr::from_text(expr).unwrap();

            let metadata = test_utils::get_metadata();

            let compiler = RibCompiler::new(RibCompilerConfig::new(metadata, vec![], vec![]));
            let error_msg = compiler.compile(expr).unwrap_err().to_string();

            let expected = r#"
            error in the following rib found at line 2, column 51
            `(1, 2)`
            cause: ambiguous types: `list<number>`, `tuple<number, number>`
            "#;

            assert_eq!(error_msg, test_utils::strip_spaces(expected));
        }

        #[test]
        fn test_invalid_function_call5() {
            let expr = r#"
            let x = {a: "foo"};
          let result = foo({a: {aa: 1, ab: 2, ac: x, ad: {ada: 1}, ae: (1, "foo")}, b: 2, c: [1, 2], d: {da: 1}});
          result
        "#;

            let expr = Expr::from_text(expr).unwrap();

            let metadata = test_utils::get_metadata();

            let compiler = RibCompiler::new(RibCompilerConfig::new(metadata, vec![], vec![]));
            let error_msg = compiler.compile(expr).unwrap_err().to_string();

            let expected = r#"
            error in the following rib found at line 2, column 21
            `{a: "foo"}`
            cause: ambiguous types: `list<number>`, `record{a: string}`
            "#;

            assert_eq!(error_msg, test_utils::strip_spaces(expected));
        }

        #[test]
        fn test_invalid_function_call6() {
            let expr = r#"
          let worker = instance("my-worker");
          let result = worker.foo({a: {aa: "foo", ab: 2, ac: [1, 2], ad: {ada: "1"}, ae: (1, "foo")}, b: 3, c: [1, 2, 3], d: {da: 4}});
          result
        "#;

            let expr = Expr::from_text(expr).unwrap();

            let metadata = test_utils::get_metadata();

            let compiler = RibCompiler::new(RibCompilerConfig::new(metadata, vec![], vec![]));
            let error_msg = compiler.compile(expr).unwrap_err().to_string();

            let expected = r#"
            error in the following rib found at line 3, column 44
            `"foo"`
            cause: type mismatch. expected s32, found string
            the expression `"foo"` is inferred as `string` by default
            "#;

            assert_eq!(error_msg, test_utils::strip_spaces(expected));
        }

        #[test]
        fn test_invalid_function_call7() {
            let expr = r#"
          let worker = instance();
          let result = worker.foo({a: {aa: 1, ab: 2, ac: [1, 2], ad: {ada: "1"}, ae: (1, "foo")}, b: 3, c: [1, 2, 3], d: {da: 4}});
          result
        "#;

            let expr = Expr::from_text(expr).unwrap();

            let metadata = test_utils::get_metadata();

            let compiler = RibCompiler::new(RibCompilerConfig::new(metadata, vec![], vec![]));
            let error_msg = compiler.compile(expr).unwrap_err().to_string();

            let expected = r#"
            error in the following rib found at line 3, column 76
            `"1"`
            cause: type mismatch. expected s32, found string
            the expression `"1"` is inferred as `string` by default
            "#;

            assert_eq!(error_msg, test_utils::strip_spaces(expected));
        }

        #[test]
        fn test_invalid_function_call8() {
            let expr = r#"
            let worker = instance("my-worker");
            let bar = {a: {ac: 1}};
            worker.foo(bar)
        "#;

            let expr = Expr::from_text(expr).unwrap();

            let metadata = test_utils::get_metadata();

            let compiler = RibCompiler::new(RibCompilerConfig::new(metadata, vec![], vec![]));
            let error_msg = compiler.compile(expr).unwrap_err().to_string();

            let expected = r#"
            error in the following rib found at line 3, column 32
            `1`
            cause: type mismatch. expected list<s32>, found s32
            the expression `1` is inferred as `s32` by default
            "#;

            assert_eq!(error_msg, test_utils::strip_spaces(expected));
        }

        #[test]
        fn test_invalid_function_call9() {
            let expr = r#"
          let worker = instance("my-worker");
          let result = worker.foo({a: {aa: 1, ab: 2, ac: [1, 2], ad: {ada: 1}, ae: (1, 2)}, b: 3, c: [1, 2, 3], d: {da: 4}});
          result
        "#;

            let expr = Expr::from_text(expr).unwrap();

            let metadata = test_utils::get_metadata();

            let compiler = RibCompiler::new(RibCompilerConfig::new(metadata, vec![], vec![]));
            let error_msg = compiler.compile(expr).unwrap_err().to_string();

            let expected = r#"
            error in the following rib found at line 3, column 88
            `2`
            cause: type mismatch. expected string, found s32
            the expression `2` is inferred as `s32` by default
            "#;

            assert_eq!(error_msg, test_utils::strip_spaces(expected));
        }

        #[test]
        fn test_invalid_function_call10() {
            let expr = r#"
          let result = foo({a: {aa: 1, ab: 2, ac: [1, 2], ad: {ada: 1}, ae: (1, 2)}, b: 3, c: [1, 2, 3]});
          result
        "#;

            let expr = Expr::from_text(expr).unwrap();

            let metadata = test_utils::get_metadata();

            let compiler = RibCompiler::new(RibCompilerConfig::new(metadata, vec![], vec![]));
            let error_msg = compiler.compile(expr).unwrap_err().to_string();

            let expected = r#"
            error in the following rib found at line 2, column 28
            `{a: {aa: 1, ab: 2, ac: [1, 2], ad: {ada: 1}, ae: (1, 2)}, b: 3, c: [1, 2, 3]}`
            cause: invalid argument to the function `foo`.  missing field(s) in record: `d`
            "#;

            assert_eq!(error_msg, test_utils::strip_spaces(expected));
        }

        #[test]
        fn test_invalid_function_call11() {
            let expr = r#"
          let result = foo({a: {aa: 1, ab: 2, ac: [1, 2], ad: {ad: 1}, ae: (1, 2)}, b: 3, c: [1, 2, 3], d: {da: 4}});
          result
        "#;

            let expr = Expr::from_text(expr).unwrap();

            let metadata = test_utils::get_metadata();

            let compiler = RibCompiler::new(RibCompilerConfig::new(metadata, vec![], vec![]));
            let error_msg = compiler.compile(expr).unwrap_err().to_string();

            let expected = r#"
            error in the following rib found at line 2, column 28
            `{a: {aa: 1, ab: 2, ac: [1, 2], ad: {ad: 1}, ae: (1, 2)}, b: 3, c: [1, 2, 3], d: {da: 4}}`
            cause: invalid argument to the function `foo`.  missing field(s) in record: `a.ad.ada`
            "#;

            assert_eq!(error_msg, test_utils::strip_spaces(expected));
        }

        #[test]
        fn test_invalid_function_call12() {
            let expr = r#"
          let result = foo({aa: {aa: 1, ab: 2, ac: [1, 2], ad: {ad: 1}, ae: (1, 2)}, b: 3, c: [1, 2, 3], d: {da: 4}});
          result
        "#;

            let expr = Expr::from_text(expr).unwrap();

            let metadata = test_utils::get_metadata();

            let compiler = RibCompiler::new(RibCompilerConfig::new(metadata, vec![], vec![]));
            let error_msg = compiler.compile(expr).unwrap_err().to_string();

            let expected = r#"
            error in the following rib found at line 2, column 28
            `{aa: {aa: 1, ab: 2, ac: [1, 2], ad: {ad: 1}, ae: (1, 2)}, b: 3, c: [1, 2, 3], d: {da: 4}}`
            cause: invalid argument to the function `foo`.  missing field(s) in record: `a`
            "#;

            assert_eq!(error_msg, test_utils::strip_spaces(expected));
        }

        #[test]
        fn test_invalid_function_call13() {
            let expr = r#"
            let aa = 1;
          let result = foo({aa: 1});
          result
        "#;

            let expr = Expr::from_text(expr).unwrap();

            let metadata = test_utils::get_metadata();

            let compiler = RibCompiler::new(RibCompilerConfig::new(metadata, vec![], vec![]));
            let error_msg = compiler.compile(expr).unwrap_err().to_string();

            let expected = r#"
            error in the following rib found at line 3, column 28
            `{aa: 1}`
            cause: invalid argument to the function `foo`.  missing field(s) in record: `a, b, c, d`
            "#;

            assert_eq!(error_msg, test_utils::strip_spaces(expected));
        }

        #[test]
        async fn test_invalid_resource_constructor_call0() {
            let expr = r#"
          let worker = instance("my-worker");
          let x = worker.cart()
        "#;
            let expr = Expr::from_text(expr).unwrap();
            let component_metadata = test_utils::get_metadata();

            let compiler_config = RibCompilerConfig::new(component_metadata, vec![], vec![]);
            let compiler = RibCompiler::new(compiler_config);
            let error_message = compiler.compile(expr).unwrap_err().to_string();

            let expected = r#"
            error in the following rib found at line 3, column 19
            `worker.cart()`
            cause: invalid argument size for function `cart`. expected 1 arguments, found 0
            "#;

            assert_eq!(error_message, strip_spaces(expected));
        }

        #[test]
        async fn test_invalid_resource_constructor_call1() {
            let expr = r#"
          let worker = instance("my-worker");
          let x = worker.cart(1)
        "#;
            let expr = Expr::from_text(expr).unwrap();
            let component_metadata = test_utils::get_metadata();

            let compiler_config = RibCompilerConfig::new(component_metadata, vec![], vec![]);
            let compiler = RibCompiler::new(compiler_config);
            let error_message = compiler.compile(expr).unwrap_err().to_string();

            let expected = r#"
            error in the following rib found at line 3, column 31
            `1`
            cause: type mismatch. expected string, found s32
            invalid argument to the function `cart`
            "#;

            assert_eq!(error_message, strip_spaces(expected));
        }

        #[test]
        async fn test_invalid_resource_method_call0() {
            let expr = r#"
          let worker = instance("my-worker");
          let x = worker.cart("foo");
          x.add-item(1)
        "#;
            let expr = Expr::from_text(expr).unwrap();
            let component_metadata = test_utils::get_metadata();

            let compiler_config = RibCompilerConfig::new(component_metadata, vec![], vec![]);
            let compiler = RibCompiler::new(compiler_config);
            let error_message = compiler.compile(expr).unwrap_err().to_string();

            let expected = r#"
            error in the following rib found at line 4, column 22
            `1`
            cause: type mismatch. expected record { product-id: string, name: string, price: f32, quantity: u32 }, found s32
            invalid argument to the function `add-item`
            "#;

            assert_eq!(error_message, strip_spaces(expected));
        }

        #[test]
        async fn test_invalid_type_parameter0() {
            let expr = r#"
          let worker = instance[golem:it2]("my-worker");
          let x = worker.cart("foo");
          x.add-item(1)
        "#;
            let expr = Expr::from_text(expr).unwrap();
            let component_metadata = test_utils::get_metadata();

            let compiler_config = RibCompilerConfig::new(component_metadata, vec![], vec![]);
            let compiler = RibCompiler::new(compiler_config);
            let error_message = compiler.compile(expr).unwrap_err().to_string();

            let expected = r#"
            error in the following rib found at line 2, column 24
            `instance[golem:it2]("my-worker")`
            cause: failed to create instance: package `golem:it2` not found
            "#;

            assert_eq!(error_message, strip_spaces(expected));
        }

        #[test]
        async fn test_invalid_type_parameter1() {
            let expr = r#"
          let worker = instance[golem:it/api2]("my-worker");
          let x = worker.cart("foo");
          x.add-item(1)
        "#;
            let expr = Expr::from_text(expr).unwrap();
            let component_metadata = test_utils::get_metadata();

            let compiler_config = RibCompilerConfig::new(component_metadata, vec![], vec![]);
            let compiler = RibCompiler::new(compiler_config);
            let error_message = compiler.compile(expr).unwrap_err().to_string();

            let expected = r#"
            error in the following rib found at line 2, column 24
            `instance[golem:it/api2]("my-worker")`
            cause: failed to create instance: `golem:it/api2` not found
            "#;

            assert_eq!(error_message, strip_spaces(expected));
        }

        #[test]
        async fn test_invalid_type_parameter2() {
            let expr = r#"
          let worker = instance[api2]("my-worker");
          let x = worker.cart("foo");
          x.add-item(1)
        "#;
            let expr = Expr::from_text(expr).unwrap();
            let component_metadata = test_utils::get_metadata();

            let compiler_config = RibCompilerConfig::new(component_metadata, vec![], vec![]);
            let compiler = RibCompiler::new(compiler_config);
            let error_message = compiler.compile(expr).unwrap_err().to_string();

            let expected = r#"
            error in the following rib found at line 2, column 24
            `instance[api2]("my-worker")`
            cause: failed to create instance: interface `api2` not found
            "#;

            assert_eq!(error_message, strip_spaces(expected));
        }
    }

    mod test_utils {
        use crate::{ComponentDependency, ComponentDependencyKey};
        use golem_wasm::analysis::analysed_type::{
            case, f32, field, handle, list, record, s32, str, tuple, u32, u64, variant,
        };
        use golem_wasm::analysis::{
            AnalysedExport, AnalysedFunction, AnalysedFunctionParameter, AnalysedFunctionResult,
            AnalysedInstance, AnalysedResourceId, AnalysedResourceMode, NameTypePair,
        };
        use uuid::Uuid;

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

        pub(crate) fn get_metadata() -> Vec<ComponentDependency> {
            let function_export = AnalysedExport::Function(AnalysedFunction {
                name: "foo".to_string(),
                parameters: vec![AnalysedFunctionParameter {
                    name: "arg1".to_string(),
                    typ: record(vec![
                        NameTypePair {
                            name: "a".to_string(),
                            typ: record(vec![
                                NameTypePair {
                                    name: "aa".to_string(),
                                    typ: s32(),
                                },
                                NameTypePair {
                                    name: "ab".to_string(),
                                    typ: s32(),
                                },
                                NameTypePair {
                                    name: "ac".to_string(),
                                    typ: list(s32()),
                                },
                                NameTypePair {
                                    name: "ad".to_string(),
                                    typ: record(vec![NameTypePair {
                                        name: "ada".to_string(),
                                        typ: s32(),
                                    }]),
                                },
                                NameTypePair {
                                    name: "ae".to_string(),
                                    typ: tuple(vec![s32(), str()]),
                                },
                            ]),
                        },
                        NameTypePair {
                            name: "b".to_string(),
                            typ: u64(),
                        },
                        NameTypePair {
                            name: "c".to_string(),
                            typ: list(s32()),
                        },
                        NameTypePair {
                            name: "d".to_string(),
                            typ: record(vec![NameTypePair {
                                name: "da".to_string(),
                                typ: s32(),
                            }]),
                        },
                    ]),
                }],
                result: Some(AnalysedFunctionResult { typ: str() }),
            });

            let resource_export = AnalysedExport::Instance(AnalysedInstance {
                name: "golem:it/api".to_string(),
                functions: vec![
                    AnalysedFunction {
                        name: "[constructor]cart".to_string(),
                        parameters: vec![AnalysedFunctionParameter {
                            name: "cons".to_string(),
                            typ: str(),
                        }],
                        result: Some(AnalysedFunctionResult {
                            typ: handle(AnalysedResourceId(0), AnalysedResourceMode::Owned),
                        }),
                    },
                    AnalysedFunction {
                        name: "[method]cart.add-item".to_string(),
                        parameters: vec![
                            AnalysedFunctionParameter {
                                name: "self".to_string(),
                                typ: handle(AnalysedResourceId(0), AnalysedResourceMode::Borrowed),
                            },
                            AnalysedFunctionParameter {
                                name: "item".to_string(),
                                typ: record(vec![
                                    field("product-id", str()),
                                    field("name", str()),
                                    field("price", f32()),
                                    field("quantity", u32()),
                                ]),
                            },
                        ],
                        result: None,
                    },
                    AnalysedFunction {
                        name: "[method]cart.remove-item".to_string(),
                        parameters: vec![
                            AnalysedFunctionParameter {
                                name: "self".to_string(),
                                typ: handle(AnalysedResourceId(0), AnalysedResourceMode::Borrowed),
                            },
                            AnalysedFunctionParameter {
                                name: "product-id".to_string(),
                                typ: str(),
                            },
                        ],
                        result: None,
                    },
                    AnalysedFunction {
                        name: "[method]cart.update-item-quantity".to_string(),
                        parameters: vec![
                            AnalysedFunctionParameter {
                                name: "self".to_string(),
                                typ: handle(AnalysedResourceId(0), AnalysedResourceMode::Borrowed),
                            },
                            AnalysedFunctionParameter {
                                name: "product-id".to_string(),
                                typ: str(),
                            },
                            AnalysedFunctionParameter {
                                name: "quantity".to_string(),
                                typ: u32(),
                            },
                        ],
                        result: None,
                    },
                    AnalysedFunction {
                        name: "[method]cart.checkout".to_string(),
                        parameters: vec![AnalysedFunctionParameter {
                            name: "self".to_string(),
                            typ: handle(AnalysedResourceId(0), AnalysedResourceMode::Borrowed),
                        }],
                        result: Some(AnalysedFunctionResult {
                            typ: variant(vec![
                                case("error", str()),
                                case("success", record(vec![field("order-id", str())])),
                            ]),
                        }),
                    },
                    AnalysedFunction {
                        name: "[method]cart.get-cart-contents".to_string(),
                        parameters: vec![AnalysedFunctionParameter {
                            name: "self".to_string(),
                            typ: handle(AnalysedResourceId(0), AnalysedResourceMode::Borrowed),
                        }],
                        result: Some(AnalysedFunctionResult {
                            typ: list(record(vec![
                                field("product-id", str()),
                                field("name", str()),
                                field("price", f32()),
                                field("quantity", u32()),
                            ])),
                        }),
                    },
                    AnalysedFunction {
                        name: "[method]cart.merge-with".to_string(),
                        parameters: vec![
                            AnalysedFunctionParameter {
                                name: "self".to_string(),
                                typ: handle(AnalysedResourceId(0), AnalysedResourceMode::Borrowed),
                            },
                            AnalysedFunctionParameter {
                                name: "other-cart".to_string(),
                                typ: handle(AnalysedResourceId(0), AnalysedResourceMode::Borrowed),
                            },
                        ],
                        result: None,
                    },
                    AnalysedFunction {
                        name: "[drop]cart".to_string(),
                        parameters: vec![AnalysedFunctionParameter {
                            name: "self".to_string(),
                            typ: handle(AnalysedResourceId(0), AnalysedResourceMode::Owned),
                        }],
                        result: None,
                    },
                ],
            });

            let component_dependency = ComponentDependency {
                component_dependency_key: ComponentDependencyKey {
                    component_name: "some_name".to_string(),
                    component_id: Uuid::new_v4(),
                    component_revision: 0,
                    root_package_name: None,
                    root_package_version: None,
                },
                component_exports: vec![function_export, resource_export],
            };

            vec![component_dependency]
        }
    }
}
