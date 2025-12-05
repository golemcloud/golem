test_r::enable!();

use test_r::test;

use golem_wasm::analysis::analysed_type::{bool, f32, f64, field, record, s16, s32, str, u64, u8};
use golem_wasm::analysis::AnalysedType;
use golem_wasm::ValueAndType;
use rib::{
    EvaluatedFnArgs, EvaluatedFqFn, EvaluatedWorkerName, Expr, Interpreter, RibCompiler,
    RibCompilerConfig, RibComponentFunctionInvoke, RibFunctionInvokeResult, RibInput,
};

#[test]
async fn test_rib_regression() {
    let expr = r#"
              let worker = instance();
              let str1: string = request.body.name;
              let str2: string = request.headers.name;
              let str3: string = request.path.name;

              let unused = worker.function-unit-response(str1);

              let str_output = worker.function-no-arg();

              let unused = worker.function-no-arg-unit();

              let str_response = worker.function-str-response(str_output);

              let number_response = worker.function-number-response(str1);

              let some_str_response = worker.function-some-str-response(str2);

              let none_str_response = worker.function-none-str-response(str2);

              let some_number_response = worker.function-some-number-response(str1);

              let none_number_response = worker.function-none-number-response(str1);

              let some_option_response = worker.function-some-option-response(str1);

              let none_option_response = worker.function-none-option-response(str1);

              let some_variant_response = worker.function-some-variant-response(str1);

              let none_variant_response = worker.function-none-variant-response(str1);

              let some_enum_response = worker.function-some-enum-response(str1);

              let none_enum_response = worker.function-none-enum-response(str1);

              let some_tuple_response = worker.function-some-tuple-response(str1);

              let none_tuple_response = worker.function-none-tuple-response(str1);

              let some_record_response = worker.function-some-record-response(str1);

              let none_record_response = worker.function-none-record-response(str1);

              let some_list_response = worker.function-some-list-response(str1);

              let none_list_response = worker.function-none-list-response(str1);

              let list_number_response = worker.function-list-number-response(str1);

              let list_str_response = worker.function-list-str-response(str1);

              let list_option_response = worker.function-list-option-response(str1);

              let list_list_response = worker.function-list-list-response(str1);

              let list_variant_response = worker.function-list-variant-response(str1);

              let list_enum_response = worker.function-list-enum-response(str1);

              let list_tuple_response = worker.function-list-tuple-response(str1);

              let list_record_response = worker.function-list-record-response(str1);

              let ok_of_str_response = worker.function-ok-str-response(str1);

              let err_of_str_response = worker.function-err-str-response(str1);

              let ok_of_number_response = worker.function-ok-number-response(str1);

              let err_of_number_response = worker.function-err-number-response(str1);

              let ok_of_variant_response = worker.function-ok-variant-response(str1);

              let err_of_variant_response = worker.function-err-variant-response(str1);

              let ok_of_enum_response = worker.function-ok-enum-response(str1);

              let err_of_enum_response = worker.function-err-enum-response(str1);

              let ok_of_tuple_response = worker.function-ok-tuple-response(str1);

              let err_of_tuple_response = worker.function-err-tuple-response(str1);

              let ok_of_flag_response = worker.function-ok-flag-response(str1);

              let err_of_flag_response = worker.function-err-flag-response(str1);

              let ok_of_record_response = worker.function-ok-record-response(str1);

              let err_of_record_response = worker.function-err-record-response(str1);

              let ok_of_list_response = worker.function-ok-list-response(str1);

              let err_of_list_response = worker.function-err-list-response(str1);

              let tuple_response = worker.function-tuple-response(str1);

              let enum_response = worker.function-enum-response(str1);

              let flag_response = worker.function-flag-response(str1);

              let variant_response = worker.function-variant-response(str1);

              let str_response_processed = str_response == "foo";

              let number_response_processed = if number_response == 42u64 then "foo" else "bar";

              let some_str_response_processed = match some_str_response {
                some(text) => text,
                none => "not found"
              };

              let none_str_response_processed = match none_str_response {
                some(text) => text,
                none => "not found"
              };


              let some_number_response_processed = match some_number_response {
                some(number) => number,
                none => 0
              };

               let none_number_response_processed = match none_number_response {
                some(number) => number,
                none => 0
              };

              let some_option_response_processed = match some_option_response {
                 some(some(x)) => x,
                 some(none) => "not found",
                 none => "not found"
              };

              let none_option_response_processed = match none_option_response {
                 some(some(x)) => x,
                 some(none) => "not found",
                 none => "not found"
              };

              let some_variant_response_processed = match some_variant_response {
                 some(case-str(_)) => "found",
                 _ => "not found"
              };

               let none_variant_response_processed = match none_variant_response {
                 some(case-str(_)) => "found",
                 _ => "not found"
              };

              let some_enum_response_processed = match some_enum_response {
                 some(enum-a) => "a",
                 some(enum-b) => "b",
                 _ => "not found"
              };

              let none_enum_response_processed = match none_enum_response {
                 some(enum-a) => "a",
                 some(enum-b) => "b",
                 _ => "not found"
              };

              let some_tuple_response_processed = match some_tuple_response {
                    some((text, _, _, _, _, _, _, _, _, _, _, _)) => text,
                     _ => "not found"
                };

               let none_tuple_response_processed = match none_tuple_response {
                    some((text, _, _, _, _, _, _, _, _, _, _, _)) => text,
                     _ => "not found"
                };


              let some_record_response_processed = match some_record_response {
                  some({data-body: {list-of-str : mylist}}) => mylist[0],
                   _ => "not found"
              };

              let none_record_response_processed = match none_record_response {
                  some({data-body: {list-of-str : mylist}}) => mylist[0],
                   _ => "not found"
              };

              let some_list_response_processed = match some_list_response {
                    some([foo]) => foo,
                     _ => "not found"
                };

               let none_list_response_processed = match none_list_response {
                    some([foo]) => foo,
                     _ => "not found"
                };

              let list_number_response_processed = match list_number_response {
                    [number] => if number > 10u64 then "greater" else "lesser",
                     _ => "not found"
                };

              let list_str_response_processed = match list_str_response {
                [text] => text,
                _ => "not found"
              };


              let list_option_response_processed = match list_option_response {
                [some(text)] => text,
                _ => "not found"
              };


              let list_list_response_processed = match list_list_response {
                 [[text]] => text,
                  _ => "not found"
              };


              let list_variant_response_processed = match list_variant_response {
                 [case-str(text)] => text,
                  _ => "not found"
              };

              let list_enum_response_processed = match list_enum_response {
                [enum-a] => "a",
                [enum-b] => "b",
                _ => "not found"
              };

              let list_tuple_response_processed1 = match list_tuple_response {
                [(text, _, _, _, _, _, _, _, _, _, _, _)] => text,
                _ => "not found"
              };


              let list_tuple_response_processed2 = match list_tuple_response {
                [(_, number, _, _, _, _, _, _, _, _, _, _)] => number,
                _ => 0
              };

              let list_tuple_response_processed3 = match list_tuple_response {
                [(_, _, number, _, _, _, _, _, _, _, _, _)] => number,
                _ => 0
              };

              let list_tuple_response_processed4 = match list_tuple_response {
                [(_, _, _, number, _, _, _, _, _, _, _, _)] => number,
                _ => 0
              };

              let list_tuple_response_processed5 = match list_tuple_response {
                [(_, _, _, _, number, _, _, _, _, _, _, _)] => number,
                _ => 0
              };

              let list_tuple_response_processed6 = match list_tuple_response {
                [(_, _, _, _, _, boolean, _, _, _, _, _, _)] => boolean,
                _ => false
              };

              let list_tuple_response_processed7 = match list_tuple_response {
                [(_, _, _, _, _, _, char, _, _, _, _, _)] => "${char}",
                _ => "not found"
              };

              let list_tuple_response_processed8 = match list_tuple_response {
                [(_, _, _, _, _, _, _, some(number), _, _, _, _)] => number,
                _ => 0
              };

              let list_tuple_response_processed9 = match list_tuple_response {
                [(_, _, _, _, _, _, _, _, ok(number), _, _, _)] => number,
                _ => 0
              };

              let list_tuple_response_processed10 = match list_tuple_response {
                [(_, _, _, _, _, _, _, _, _, [boolean], _, _)] => boolean,
                _ => false
              };

              let list_tuple_response_processed11 = match list_tuple_response {
                [(_, _, _, _, _, _, _, _, _, _, case-hello(number), _)] => number,
                _ => 0
              };

              let list_tuple_response_processed12 = match list_tuple_response {
                [(_, _, _, _, _, _, _, _, _, _, _, {field-one: boolean, field-two: text})] => "${boolean}-${text}",
                _ => "not found"
              };


              let list_record_response_processed = match list_record_response {
                [{data-body: {list-of-str : [text]}}] => text,
                _ => "not found"
              };

              let ok_of_str_response_processed = match ok_of_str_response {
                ok(text) => text,
                err(msg) => msg
              };

              let err_of_str_response_processed = match err_of_str_response {
                ok(text) => text,
                err(msg) => msg
              };

              let ok_of_number_response_processed = match ok_of_number_response {
                ok(number) => number,
                err(number) => number
              };

              let err_of_number_response_processed = match err_of_number_response {
                  ok(number) => number,
                  err(number) => number
               };

              let ok_of_variant_response_processed = match ok_of_variant_response {
                ok(case-str(a)) => a,
                err(case-str(b)) => b,
                _ => "not found"
              };

                let err_of_variant_response_processed = match err_of_variant_response {
                    ok(case-str(a)) => a,
                    err(case-str(b)) => b,
                    _ => "not found"
                };

              let ok_of_enum_response_processed = match ok_of_enum_response {
                ok(enum-a) => "a",
                ok(enum-b) => "b",
                ok(enum-c) => "c",
                err(msg) => "not found"
              };

                let err_of_enum_response_processed = match err_of_enum_response {
                    ok(enum-a) => "a",
                    ok(enum-b) => "b",
                    ok(enum-c) => "c",
                    err(enum-a) => "error-a",
                    err(enum-b) => "error-b",
                    err(enum-c) => "error-c"
                };

              let ok_of_tuple_response_processed = match ok_of_tuple_response {
                ok((text, _, _, _, _, _, _, _, _, _, _, _)) => text,
                err(msg) => "not found"
              };

               let err_of_tuple_response_processed = match err_of_tuple_response {
                ok((text, _, _, _, _, _, _, _, _, _, _, _)) => text,
                err((text, _, _, _, _, _, _, _, _, _, _, _)) => text
              };


              let ok_of_flag_response_processed = match ok_of_flag_response {
                ok({featurex, featurey, featurez}) => "found all flags",
                ok({featurex}) => "found x",
                ok({featurey}) => "found x",
                ok({featurex, featurey}) => "found x and y",
                err({featurex, featurey, featurez}) => "found all flags",
                err({featurex}) => "found x",
                err({featurey}) => "found x",
                err({featurex, featurey}) => "found x and y"
               };

                let err_of_flag_response_processed = match err_of_flag_response {
                ok({featurex, featurey, featurez}) => "found all flags",
                ok({featurex}) => "found x",
                ok({featurey}) => "found x",
                ok({featurex, featurey}) => "found x and y",
                err({featurex, featurey, featurez}) => "found all flags",
                err({featurex}) => "found x",
                err({featurey}) => "found x",
                err({featurex, featurey}) => "found x and y"
               };

              let ok_of_record_response_processed = match ok_of_record_response {
                 ok({data-body: {list-of-str : mylist}}) => mylist[0],
                 err(msg) => "not found"
               };

               let err_of_record_response_processed = match err_of_record_response {
                 ok({data-body: {list-of-str : mylist}}) => mylist[0],
                 err({data-body: {list-of-str : mylist}}) => mylist[0]
               };

               let tuple_response_processed = match tuple_response {
                 (_, _, _, _, _, _, _, _, _, _, case-hello(a), _) => "${a}"
               };

               let enum_response_processed = match enum_response {
                 enum-a => "a",
                 enum-b => "b",
                 enum-c => "c",
                 _ => "not found"
               };

               let variant_response_processed = match variant_response {
                 case-str(text) => text,
                 _ => "not found"
               };

              {
                 a : some_str_response_processed,
                 aa: list_tuple_response_processed2,
                 ab: list_tuple_response_processed3,
                 ac: list_tuple_response_processed4,
                 ad: list_tuple_response_processed5,
                 ae: list_tuple_response_processed6,
                 af: list_tuple_response_processed7,
                 ag: list_tuple_response_processed8,
                 ah: list_tuple_response_processed9,
                 ai: list_tuple_response_processed10,
                 aj: list_tuple_response_processed11,
                 ak: list_tuple_response_processed12,
                 b: some_number_response_processed,
                 bb: none_number_response_processed,
                 c: some_option_response_processed,
                 cc: none_option_response_processed,
                 d: some_variant_response_processed,
                 dd: none_variant_response_processed,
                 e: some_enum_response_processed,
                 ee: none_enum_response_processed,
                 f: some_tuple_response_processed,
                 ff: none_tuple_response_processed,
                 g: some_record_response_processed,
                 gg: none_record_response_processed,
                 h: some_list_response_processed,
                 hh: none_list_response_processed,
                 i: list_number_response_processed,
                 j: list_str_response_processed,
                 k: list_option_response_processed,
                 l: list_list_response_processed,
                 m: list_variant_response_processed,
                 n: list_enum_response_processed,
                 o: list_tuple_response_processed1,
                 p: list_record_response_processed,
                 q: ok_of_str_response_processed,
                 qq: err_of_str_response_processed,
                 r: ok_of_number_response_processed,
                 rr: err_of_number_response_processed,
                 s: ok_of_variant_response_processed,
                 ss: err_of_variant_response_processed,
                 t: ok_of_enum_response_processed,
                 tt: err_of_enum_response_processed,
                 u: ok_of_tuple_response_processed,
                 uu: err_of_tuple_response_processed,
                 v: ok_of_flag_response_processed,
                 vv: err_of_flag_response_processed,
                 w: ok_of_record_response_processed,
                 ww: err_of_record_response_processed,
                 x: tuple_response_processed,
                 y: enum_response_processed,
                 z: variant_response_processed
              }
        "#;

    let expr = Expr::from_text(expr).unwrap();

    let compiler = RibCompiler::new(RibCompilerConfig::new(
        component_metadata::component_metadata(),
        vec![],
        vec![],
    ));

    use std::time::Instant;

    let start = Instant::now();

    let compiled_expr = compiler.compile(expr).unwrap().byte_code;

    let duration = start.elapsed();

    println!("Compile time of the complex rib: {duration:?}");

    let mut rib_executor = mock_interpreter::interpreter();

    let start = Instant::now();

    let result = rib_executor.run(compiled_expr).await.unwrap();

    let duration = start.elapsed();

    println!("Execution time of the complex rib: {duration:?}");

    let actual_as_text = test_utils::convert_value_and_type_to_str(&result.get_val().unwrap());

    let expected_as_text = test_utils::convert_value_and_type_to_str(&expected_value_and_type());

    assert_eq!(
        result.get_val().unwrap(),
        expected_value_and_type(),
        "Assertion failed! \n\n Actual value as string  : {actual_as_text} \n\n Expected value as string: {expected_as_text}\n"
    );
}

fn expected_value_and_type() -> ValueAndType {
    let wasm_wave_str = r#"
          {
            a: "foo",
            b: 42,
            bb: 0,
            c: "foo",
            cc: "not found",
            d: "found",
            dd: "not found",
            e: "a",
            ee: "not found",
            f: "foo",
            ff: "not found",
            g: "foo",
            gg: "not found",
            h: "foo",
            hh: "not found",
            i: "greater",
            j: "foo",
            k: "foo",
            l: "foo",
            m: "foo",
            n: "a",
            o: "foo",
            p: "foo",
            q: "foo",
            qq: "foo",
            r: 42,
            rr: 42,
            s: "foo",
            ss: "foo",
            t: "a",
            tt: "error-a",
            u: "foo",
            uu: "foo",
            v: "found x",
            vv: "found x",
            w: "foo",
            ww: "foo",
            x: "42",
            y: "a",
            z: "foo",
            aa: 42,
            ab: 42,
            ac: 42,
            ad: 42,
            ae: true,
            af: "a",
            ag: 42,
            ah: 42,
            ai: true,
            aj: 42,
            ak: "true-foo"
          }
        "#;

    test_utils::get_value_and_type(&expected_analysed_type(), wasm_wave_str)
}

fn expected_analysed_type() -> AnalysedType {
    record(vec![
        field("a", str()),
        field("aa", u64()),
        field("ab", s32()),
        field("ac", f32()),
        field("ad", f64()),
        field("ae", bool()),
        field("af", str()),
        field("ag", s16()),
        field("ah", u8()),
        field("ai", bool()),
        field("aj", f64()),
        field("ak", str()),
        field("b", u64()),
        field("bb", u64()),
        field("c", str()),
        field("cc", str()),
        field("d", str()),
        field("dd", str()),
        field("e", str()),
        field("ee", str()),
        field("f", str()),
        field("ff", str()),
        field("g", str()),
        field("gg", str()),
        field("h", str()),
        field("hh", str()),
        field("i", str()),
        field("j", str()),
        field("k", str()),
        field("l", str()),
        field("m", str()),
        field("n", str()),
        field("o", str()),
        field("p", str()),
        field("q", str()),
        field("qq", str()),
        field("r", u64()),
        field("rr", u64()),
        field("s", str()),
        field("ss", str()),
        field("t", str()),
        field("tt", str()),
        field("u", str()),
        field("uu", str()),
        field("v", str()),
        field("vv", str()),
        field("w", str()),
        field("ww", str()),
        field("x", str()),
        field("y", str()),
        field("z", str()),
    ])
}

mod component_metadata {
    use crate::function_metadata;
    use rib::{ComponentDependency, ComponentDependencyKey};
    use uuid::Uuid;

    pub(crate) fn component_metadata() -> Vec<ComponentDependency> {
        let mut exports = vec![];
        exports.extend(function_metadata::function_unit_response());
        exports.extend(function_metadata::function_no_arg());
        exports.extend(function_metadata::function_no_arg_unit());
        exports.extend(function_metadata::function_str_response());
        exports.extend(function_metadata::function_number_response());
        exports.extend(function_metadata::function_some_of_str_response());
        exports.extend(function_metadata::function_none_of_str_response());
        exports.extend(function_metadata::function_some_of_number_response());
        exports.extend(function_metadata::function_none_of_number_response());
        exports.extend(function_metadata::function_some_of_option_response());
        exports.extend(function_metadata::function_none_of_option_response());
        exports.extend(function_metadata::function_some_of_variant_response());
        exports.extend(function_metadata::function_none_of_variant_response());
        exports.extend(function_metadata::function_some_of_enum_response());
        exports.extend(function_metadata::function_none_of_enum_response());
        exports.extend(function_metadata::function_some_of_tuple_response());
        exports.extend(function_metadata::function_none_of_tuple_response());
        exports.extend(function_metadata::function_some_of_record_response());
        exports.extend(function_metadata::function_none_of_record_response());
        exports.extend(function_metadata::function_some_of_list_response());
        exports.extend(function_metadata::function_none_of_list_response());
        exports.extend(function_metadata::function_list_of_number_response());
        exports.extend(function_metadata::function_list_of_str_response());
        exports.extend(function_metadata::function_list_of_option_response());
        exports.extend(function_metadata::function_list_of_list_response());
        exports.extend(function_metadata::function_list_of_variant_response());
        exports.extend(function_metadata::function_list_of_enum_response());
        exports.extend(function_metadata::function_list_of_tuple_response());
        exports.extend(function_metadata::function_list_of_record_response());
        exports.extend(function_metadata::function_ok_of_str_response());
        exports.extend(function_metadata::function_err_of_str_response());
        exports.extend(function_metadata::function_ok_of_number_response());
        exports.extend(function_metadata::function_err_of_number_response());
        exports.extend(function_metadata::function_ok_of_option_response());
        exports.extend(function_metadata::function_err_of_option_response());
        exports.extend(function_metadata::function_ok_of_variant_response());
        exports.extend(function_metadata::function_err_of_variant_response());
        exports.extend(function_metadata::function_ok_of_enum_response());
        exports.extend(function_metadata::function_err_of_enum_response());
        exports.extend(function_metadata::function_ok_of_tuple_response());
        exports.extend(function_metadata::function_err_of_tuple_response());
        exports.extend(function_metadata::function_ok_of_flag_response());
        exports.extend(function_metadata::function_err_of_flag_response());
        exports.extend(function_metadata::function_ok_of_record_response());
        exports.extend(function_metadata::function_err_of_record_response());
        exports.extend(function_metadata::function_ok_of_list_response());
        exports.extend(function_metadata::function_err_of_list_response());
        exports.extend(function_metadata::function_tuple_response());
        exports.extend(function_metadata::function_enum_response());
        exports.extend(function_metadata::function_flag_response());
        exports.extend(function_metadata::function_variant_response());
        exports.extend(function_metadata::function_record_response());
        exports.extend(function_metadata::function_all_inputs());

        let component_info = ComponentDependencyKey {
            component_name: "foo".to_string(),
            component_id: Uuid::new_v4(),
            component_revision: 0,
            root_package_name: None,
            root_package_version: None,
        };

        vec![ComponentDependency::new(component_info, exports)]
    }
}

mod function_metadata {
    use crate::{data_types, test_utils};
    use golem_wasm::analysis::AnalysedExport;

    pub(crate) fn function_unit_response() -> Vec<AnalysedExport> {
        test_utils::get_function_component_metadata(
            "function-unit-response",
            vec![data_types::str_type()],
            None,
        )
    }

    pub(crate) fn function_no_arg() -> Vec<AnalysedExport> {
        test_utils::get_function_component_metadata(
            "function-no-arg",
            vec![],
            Some(data_types::str_type()),
        )
    }

    pub(crate) fn function_no_arg_unit() -> Vec<AnalysedExport> {
        test_utils::get_function_component_metadata("function-no-arg-unit", vec![], None)
    }

    pub(crate) fn function_str_response() -> Vec<AnalysedExport> {
        test_utils::get_function_component_metadata(
            "function-str-response",
            vec![data_types::str_type()],
            Some(data_types::str_type()),
        )
    }

    pub(crate) fn function_number_response() -> Vec<AnalysedExport> {
        test_utils::get_function_component_metadata(
            "function-number-response",
            vec![data_types::str_type()],
            Some(data_types::number_type()),
        )
    }

    pub(crate) fn function_some_of_str_response() -> Vec<AnalysedExport> {
        test_utils::get_function_component_metadata(
            "function-some-str-response",
            vec![data_types::str_type()],
            Some(data_types::option_of_str_type()),
        )
    }

    pub(crate) fn function_none_of_str_response() -> Vec<AnalysedExport> {
        test_utils::get_function_component_metadata(
            "function-none-str-response",
            vec![data_types::str_type()],
            Some(data_types::option_of_str_type()),
        )
    }

    pub(crate) fn function_some_of_number_response() -> Vec<AnalysedExport> {
        test_utils::get_function_component_metadata(
            "function-some-number-response",
            vec![data_types::str_type()],
            Some(data_types::option_of_number_type()),
        )
    }

    pub(crate) fn function_none_of_number_response() -> Vec<AnalysedExport> {
        test_utils::get_function_component_metadata(
            "function-none-number-response",
            vec![data_types::str_type()],
            Some(data_types::option_of_number_type()),
        )
    }

    pub(crate) fn function_some_of_option_response() -> Vec<AnalysedExport> {
        test_utils::get_function_component_metadata(
            "function-some-option-response",
            vec![data_types::str_type()],
            Some(data_types::option_of_option_type()),
        )
    }

    pub(crate) fn function_none_of_option_response() -> Vec<AnalysedExport> {
        test_utils::get_function_component_metadata(
            "function-none-option-response",
            vec![data_types::str_type()],
            Some(data_types::option_of_option_type()),
        )
    }

    pub(crate) fn function_some_of_variant_response() -> Vec<AnalysedExport> {
        test_utils::get_function_component_metadata(
            "function-some-variant-response",
            vec![data_types::str_type()],
            Some(data_types::option_of_variant_type()),
        )
    }

    pub(crate) fn function_none_of_variant_response() -> Vec<AnalysedExport> {
        test_utils::get_function_component_metadata(
            "function-none-variant-response",
            vec![data_types::str_type()],
            Some(data_types::option_of_variant_type()),
        )
    }

    pub(crate) fn function_some_of_enum_response() -> Vec<AnalysedExport> {
        test_utils::get_function_component_metadata(
            "function-some-enum-response",
            vec![data_types::str_type()],
            Some(data_types::option_of_enum_type()),
        )
    }

    pub(crate) fn function_none_of_enum_response() -> Vec<AnalysedExport> {
        test_utils::get_function_component_metadata(
            "function-none-enum-response",
            vec![data_types::str_type()],
            Some(data_types::option_of_enum_type()),
        )
    }

    pub(crate) fn function_some_of_tuple_response() -> Vec<AnalysedExport> {
        test_utils::get_function_component_metadata(
            "function-some-tuple-response",
            vec![data_types::str_type()],
            Some(data_types::option_of_tuple()),
        )
    }

    pub(crate) fn function_none_of_tuple_response() -> Vec<AnalysedExport> {
        test_utils::get_function_component_metadata(
            "function-none-tuple-response",
            vec![data_types::str_type()],
            Some(data_types::option_of_tuple()),
        )
    }

    pub(crate) fn function_some_of_record_response() -> Vec<AnalysedExport> {
        test_utils::get_function_component_metadata(
            "function-some-record-response",
            vec![data_types::str_type()],
            Some(data_types::option_of_record_type()),
        )
    }

    pub(crate) fn function_none_of_record_response() -> Vec<AnalysedExport> {
        test_utils::get_function_component_metadata(
            "function-none-record-response",
            vec![data_types::str_type()],
            Some(data_types::option_of_record_type()),
        )
    }

    pub(crate) fn function_some_of_list_response() -> Vec<AnalysedExport> {
        test_utils::get_function_component_metadata(
            "function-some-list-response",
            vec![data_types::str_type()],
            Some(data_types::option_of_list()),
        )
    }

    pub(crate) fn function_none_of_list_response() -> Vec<AnalysedExport> {
        test_utils::get_function_component_metadata(
            "function-none-list-response",
            vec![data_types::str_type()],
            Some(data_types::option_of_list()),
        )
    }

    pub(crate) fn function_list_of_number_response() -> Vec<AnalysedExport> {
        test_utils::get_function_component_metadata(
            "function-list-number-response",
            vec![data_types::str_type()],
            Some(data_types::list_of_number_type_type()),
        )
    }

    pub(crate) fn function_list_of_str_response() -> Vec<AnalysedExport> {
        test_utils::get_function_component_metadata(
            "function-list-str-response",
            vec![data_types::str_type()],
            Some(data_types::list_of_str_type()),
        )
    }

    pub(crate) fn function_list_of_option_response() -> Vec<AnalysedExport> {
        test_utils::get_function_component_metadata(
            "function-list-option-response",
            vec![data_types::str_type()],
            Some(data_types::list_of_option_type()),
        )
    }

    pub(crate) fn function_list_of_list_response() -> Vec<AnalysedExport> {
        test_utils::get_function_component_metadata(
            "function-list-list-response",
            vec![data_types::str_type()],
            Some(data_types::list_of_list_type()),
        )
    }

    pub(crate) fn function_list_of_variant_response() -> Vec<AnalysedExport> {
        test_utils::get_function_component_metadata(
            "function-list-variant-response",
            vec![data_types::str_type()],
            Some(data_types::list_of_variant_type()),
        )
    }

    pub(crate) fn function_list_of_enum_response() -> Vec<AnalysedExport> {
        test_utils::get_function_component_metadata(
            "function-list-enum-response",
            vec![data_types::str_type()],
            Some(data_types::list_of_enum_type()),
        )
    }

    pub(crate) fn function_list_of_tuple_response() -> Vec<AnalysedExport> {
        test_utils::get_function_component_metadata(
            "function-list-tuple-response",
            vec![data_types::str_type()],
            Some(data_types::list_of_tuple()),
        )
    }

    pub(crate) fn function_list_of_record_response() -> Vec<AnalysedExport> {
        test_utils::get_function_component_metadata(
            "function-list-record-response",
            vec![data_types::str_type()],
            Some(data_types::list_of_record_type()),
        )
    }

    pub(crate) fn function_ok_of_str_response() -> Vec<AnalysedExport> {
        test_utils::get_function_component_metadata(
            "function-ok-str-response",
            vec![data_types::str_type()],
            Some(data_types::result_of_str_type()),
        )
    }

    pub(crate) fn function_err_of_str_response() -> Vec<AnalysedExport> {
        test_utils::get_function_component_metadata(
            "function-err-str-response",
            vec![data_types::str_type()],
            Some(data_types::result_of_str_type()),
        )
    }

    pub(crate) fn function_ok_of_number_response() -> Vec<AnalysedExport> {
        test_utils::get_function_component_metadata(
            "function-ok-number-response",
            vec![data_types::str_type()],
            Some(data_types::result_of_number_type()),
        )
    }

    pub(crate) fn function_err_of_number_response() -> Vec<AnalysedExport> {
        test_utils::get_function_component_metadata(
            "function-err-number-response",
            vec![data_types::str_type()],
            Some(data_types::result_of_number_type()),
        )
    }

    pub(crate) fn function_ok_of_option_response() -> Vec<AnalysedExport> {
        test_utils::get_function_component_metadata(
            "function-ok-option-response",
            vec![data_types::str_type()],
            Some(data_types::result_of_option_type()),
        )
    }

    pub(crate) fn function_err_of_option_response() -> Vec<AnalysedExport> {
        test_utils::get_function_component_metadata(
            "function-err-option-response",
            vec![data_types::str_type()],
            Some(data_types::result_of_option_type()),
        )
    }

    pub(crate) fn function_ok_of_variant_response() -> Vec<AnalysedExport> {
        test_utils::get_function_component_metadata(
            "function-ok-variant-response",
            vec![data_types::str_type()],
            Some(data_types::result_of_variant_type()),
        )
    }

    pub(crate) fn function_err_of_variant_response() -> Vec<AnalysedExport> {
        test_utils::get_function_component_metadata(
            "function-err-variant-response",
            vec![data_types::str_type()],
            Some(data_types::result_of_variant_type()),
        )
    }

    pub(crate) fn function_ok_of_enum_response() -> Vec<AnalysedExport> {
        test_utils::get_function_component_metadata(
            "function-ok-enum-response",
            vec![data_types::str_type()],
            Some(data_types::result_of_enum_type()),
        )
    }

    pub(crate) fn function_err_of_enum_response() -> Vec<AnalysedExport> {
        test_utils::get_function_component_metadata(
            "function-err-enum-response",
            vec![data_types::str_type()],
            Some(data_types::result_of_enum_type()),
        )
    }

    pub(crate) fn function_ok_of_tuple_response() -> Vec<AnalysedExport> {
        test_utils::get_function_component_metadata(
            "function-ok-tuple-response",
            vec![data_types::str_type()],
            Some(data_types::result_of_tuple_type()),
        )
    }

    pub(crate) fn function_err_of_tuple_response() -> Vec<AnalysedExport> {
        test_utils::get_function_component_metadata(
            "function-err-tuple-response",
            vec![data_types::str_type()],
            Some(data_types::result_of_tuple_type()),
        )
    }

    pub(crate) fn function_ok_of_flag_response() -> Vec<AnalysedExport> {
        test_utils::get_function_component_metadata(
            "function-ok-flag-response",
            vec![data_types::str_type()],
            Some(data_types::result_of_flag_type()),
        )
    }

    pub(crate) fn function_err_of_flag_response() -> Vec<AnalysedExport> {
        test_utils::get_function_component_metadata(
            "function-err-flag-response",
            vec![data_types::str_type()],
            Some(data_types::result_of_flag_type()),
        )
    }

    pub(crate) fn function_ok_of_record_response() -> Vec<AnalysedExport> {
        test_utils::get_function_component_metadata(
            "function-ok-record-response",
            vec![data_types::str_type()],
            Some(data_types::result_of_record_type()),
        )
    }

    pub(crate) fn function_err_of_record_response() -> Vec<AnalysedExport> {
        test_utils::get_function_component_metadata(
            "function-err-record-response",
            vec![data_types::str_type()],
            Some(data_types::result_of_record_type()),
        )
    }

    pub(crate) fn function_ok_of_list_response() -> Vec<AnalysedExport> {
        test_utils::get_function_component_metadata(
            "function-ok-list-response",
            vec![data_types::str_type()],
            Some(data_types::result_of_list_type()),
        )
    }

    pub(crate) fn function_err_of_list_response() -> Vec<AnalysedExport> {
        test_utils::get_function_component_metadata(
            "function-err-list-response",
            vec![data_types::str_type()],
            Some(data_types::result_of_list_type()),
        )
    }

    pub(crate) fn function_tuple_response() -> Vec<AnalysedExport> {
        test_utils::get_function_component_metadata(
            "function-tuple-response",
            vec![data_types::str_type()],
            Some(data_types::tuple_type()),
        )
    }

    pub(crate) fn function_enum_response() -> Vec<AnalysedExport> {
        test_utils::get_function_component_metadata(
            "function-enum-response",
            vec![data_types::str_type()],
            Some(data_types::enum_type()),
        )
    }

    pub(crate) fn function_flag_response() -> Vec<AnalysedExport> {
        test_utils::get_function_component_metadata(
            "function-flag-response",
            vec![data_types::str_type()],
            Some(data_types::flag_type()),
        )
    }

    pub(crate) fn function_variant_response() -> Vec<AnalysedExport> {
        test_utils::get_function_component_metadata(
            "function-variant-response",
            vec![data_types::str_type()],
            Some(data_types::variant_type()),
        )
    }

    pub(crate) fn function_record_response() -> Vec<AnalysedExport> {
        test_utils::get_function_component_metadata(
            "function-record-response",
            vec![data_types::str_type()],
            Some(data_types::record_type()),
        )
    }

    pub(crate) fn function_all_inputs() -> Vec<AnalysedExport> {
        test_utils::get_function_component_metadata(
            "function-all-inputs",
            vec![
                data_types::str_type(),
                data_types::number_type(),
                data_types::option_of_str_type(),
                data_types::option_of_number_type(),
                data_types::option_of_option_type(),
                data_types::option_of_variant_type(),
                data_types::option_of_enum_type(),
                data_types::option_of_tuple(),
                data_types::option_of_record_type(),
                data_types::option_of_list(),
                data_types::list_of_number_type_type(),
                data_types::list_of_str_type(),
                data_types::list_of_option_type(),
                data_types::list_of_list_type(),
                data_types::list_of_variant_type(),
                data_types::list_of_enum_type(),
                data_types::list_of_tuple(),
                data_types::list_of_record_type(),
                data_types::result_of_str_type(),
                data_types::result_of_number_type(),
                data_types::result_of_option_type(),
                data_types::result_of_variant_type(),
                data_types::result_of_enum_type(),
                data_types::result_of_tuple_type(),
                data_types::result_of_flag_type(),
                data_types::result_of_record_type(),
                data_types::result_of_list_type(),
                data_types::tuple_type(),
                data_types::enum_type(),
                data_types::flag_type(),
                data_types::variant_type(),
                data_types::record_type(),
            ],
            Some(data_types::str_type()),
        )
    }
}

mod data_types {
    use golem_wasm::analysis::analysed_type::{
        bool, case, chr, f32, f64, field, flags, list, option, r#enum, record, result, s16, s32,
        s64, s8, str, tuple, u16, u32, u64, u8, unit_case, variant,
    };
    use golem_wasm::analysis::*;

    // Result
    pub(crate) fn result_of_str_type() -> AnalysedType {
        result(str(), str())
    }

    pub(crate) fn result_of_number_type() -> AnalysedType {
        result(u64(), u64())
    }

    pub(crate) fn result_of_option_type() -> AnalysedType {
        result(option_of_str_type(), option_of_str_type())
    }

    pub(crate) fn result_of_variant_type() -> AnalysedType {
        result(variant_type(), variant_type())
    }

    pub(crate) fn result_of_enum_type() -> AnalysedType {
        result(enum_type(), enum_type())
    }

    pub(crate) fn result_of_tuple_type() -> AnalysedType {
        result(tuple_type(), tuple_type())
    }

    pub(crate) fn result_of_flag_type() -> AnalysedType {
        result(flag_type(), flag_type())
    }

    pub(crate) fn result_of_record_type() -> AnalysedType {
        result(record_type(), record_type())
    }

    pub(crate) fn result_of_list_type() -> AnalysedType {
        result(list_of_str_type(), list_of_str_type())
    }

    // List
    pub(crate) fn list_of_number_type_type() -> AnalysedType {
        list(u64())
    }

    pub(crate) fn list_of_str_type() -> AnalysedType {
        list(str())
    }

    pub(crate) fn list_of_option_type() -> AnalysedType {
        list(option(str()))
    }

    pub(crate) fn list_of_list_type() -> AnalysedType {
        list(list(str()))
    }

    pub(crate) fn list_of_variant_type() -> AnalysedType {
        list(variant_type())
    }

    pub(crate) fn list_of_enum_type() -> AnalysedType {
        list(enum_type())
    }

    pub(crate) fn list_of_tuple() -> AnalysedType {
        list(tuple_type())
    }

    pub(crate) fn list_of_record_type() -> AnalysedType {
        list(record_type())
    }

    pub(crate) fn option_of_number_type() -> AnalysedType {
        option(u64())
    }

    // Option
    pub(crate) fn option_of_str_type() -> AnalysedType {
        option(str())
    }

    pub(crate) fn option_of_option_type() -> AnalysedType {
        option(option(str()))
    }

    pub(crate) fn option_of_variant_type() -> AnalysedType {
        option(variant_type())
    }

    pub(crate) fn option_of_enum_type() -> AnalysedType {
        option(enum_type())
    }

    pub(crate) fn option_of_tuple() -> AnalysedType {
        option(tuple_type())
    }

    pub(crate) fn option_of_record_type() -> AnalysedType {
        option(record_type())
    }

    pub(crate) fn option_of_list() -> AnalysedType {
        option(list(str()))
    }

    // Record
    pub(crate) fn record_type() -> AnalysedType {
        record(vec![
            field(
                "string-headers",
                record(vec![field("authorization-string", str())]),
            ),
            field(
                "data-body",
                record(vec![
                    field("str", str()),
                    field("list-of-str", list_of_str_type()),
                    field("list-of-option", list_of_option_type()),
                    field("list-of-list", list_of_list_type()),
                    field("list-of-variant", list_of_variant_type()),
                    field("list-of-enum", list_of_enum_type()),
                    field("list-of-tuple", list_of_tuple()),
                    field(
                        "list-of-record",
                        list(record(vec![
                            field("field-string-one", str()),
                            field("field-string-two", str()),
                        ])),
                    ),
                    field("option-of-str", option_of_str_type()),
                    field("option-of-option", option_of_option_type()),
                    field("option-of-variant", option_of_variant_type()),
                    field("option-of-enum", option_of_enum_type()),
                    field("option-of-tuple", option_of_tuple()),
                    field(
                        "option-of-record",
                        option(record(vec![
                            field("field-string-one", str()),
                            field("field-string-two", str()),
                        ])),
                    ),
                    field("option-of-list", option_of_list()),
                    field(
                        "nested-record",
                        record(vec![
                            field("field-string-one", str()),
                            field("field-string-two", str()),
                        ]),
                    ),
                    field("variant-data-a", variant_type()),
                    field("variant-data-b", variant_type()),
                    field("variant-data-c", variant_type()),
                    field("variant-data-d", variant_type()),
                    field("variant-data-e", variant_type()),
                    field("variant-data-f", variant_type()),
                    field("enum-data-a", enum_type()),
                    field("enum-data-b", enum_type()),
                    field("enum-data-c", enum_type()),
                    field("flags-data-a", flag_type()),
                    field("flags-data-b", flag_type()),
                    field("flags-data-c", flag_type()),
                    field("result-data-a", result_of_str_type()),
                    field("result-data-b", result_of_number_type()),
                    field("result-data-c", result_of_enum_type()),
                    field("result-data-d", result_of_variant_type()),
                    field("result-data-e", result_of_tuple_type()),
                    field("result-data-f", result_of_option_type()),
                    field("result-data-g", result_of_str_type()),
                    field("result-data-h", result_of_number_type()),
                    field("result-data-i", result_of_enum_type()),
                    field("result-data-j", result_of_variant_type()),
                    field("result-data-k", result_of_tuple_type()),
                    field("result-data-l", result_of_option_type()),
                    field("result-data-m", result_of_flag_type()),
                    field("result-data-n", result_of_flag_type()),
                    field("tuple-data", tuple_type()),
                    field("character-data", chr()),
                    field("f64-data", f64()),
                    field("f32-data", f32()),
                    field("u64-data", u64()),
                    field("s64-data", s64()),
                    field("u32-data", u32()),
                    field("s32-data", s32()),
                    field("u16-data", u16()),
                    field("s16-data", s16()),
                    field("u8-data", u8()),
                    field("s8-data", s8()),
                    field("boolean-data", bool()),
                ]),
            ),
        ])
    }

    // Tuple
    pub(crate) fn tuple_type() -> AnalysedType {
        tuple(vec![
            str(),
            u64(),
            s32(),
            f32(),
            f64(),
            bool(),
            chr(),
            option(s16()),
            result(u8(), s8()),
            list(bool()),
            variant(vec![case("case-hello", f64()), unit_case("case-none")]),
            record(vec![field("field-one", bool()), field("field-two", str())]),
        ])
    }

    // Enum
    pub(crate) fn enum_type() -> AnalysedType {
        r#enum(&["enum-a", "enum-b", "enum-c"])
    }

    // Str
    pub(crate) fn str_type() -> AnalysedType {
        str()
    }

    // Number
    pub(crate) fn number_type() -> AnalysedType {
        u64()
    }

    // Flag
    pub(crate) fn flag_type() -> AnalysedType {
        flags(&["featurex", "featurey", "featurez"])
    }

    // Variant
    pub(crate) fn variant_type() -> AnalysedType {
        variant(vec![
            unit_case("case-none"),
            case("case-str", str()),
            case("case-u64", u64()),
            case("case-s32", s32()),
            case("case-f32", f32()),
            case("case-f64", f64()),
            case("case-bool", bool()),
            case("case-chr", chr()),
            case("case-list", list(s16())),
            case("case-option", option(u16())),
            case("case-result", result(u8(), s8())),
            case(
                "case-record",
                record(vec![field("field1", str()), field("field2", bool())]),
            ),
            case("case-tuple", tuple(vec![f32(), u32()])),
        ])
    }
}

mod mock_data {
    use crate::{data_types, test_utils};
    use golem_wasm::ValueAndType;

    pub(crate) fn ok_of_str() -> ValueAndType {
        test_utils::get_value_and_type(&data_types::result_of_str_type(), "ok(\"foo\")")
    }

    pub(crate) fn err_of_str() -> ValueAndType {
        test_utils::get_value_and_type(&data_types::result_of_str_type(), "err(\"foo\")")
    }

    pub(crate) fn ok_of_number() -> ValueAndType {
        test_utils::get_value_and_type(&data_types::result_of_number_type(), "ok(42)")
    }

    pub(crate) fn err_of_number() -> ValueAndType {
        test_utils::get_value_and_type(&data_types::result_of_number_type(), "err(42)")
    }

    pub(crate) fn ok_of_option() -> ValueAndType {
        test_utils::get_value_and_type(&data_types::result_of_option_type(), "ok(some(\"foo\"))")
    }

    pub(crate) fn err_of_option() -> ValueAndType {
        test_utils::get_value_and_type(&data_types::result_of_option_type(), "err(some(\"foo\"))")
    }

    pub(crate) fn ok_of_variant() -> ValueAndType {
        test_utils::get_value_and_type(
            &data_types::result_of_variant_type(),
            "ok(case-str(\"foo\"))",
        )
    }

    pub(crate) fn err_of_variant() -> ValueAndType {
        test_utils::get_value_and_type(
            &data_types::result_of_variant_type(),
            "err(case-str(\"foo\"))",
        )
    }

    pub(crate) fn ok_of_enum() -> ValueAndType {
        test_utils::get_value_and_type(&data_types::result_of_enum_type(), "ok(enum-a)")
    }

    pub(crate) fn err_of_enum() -> ValueAndType {
        test_utils::get_value_and_type(&data_types::result_of_enum_type(), "err(enum-a)")
    }

    pub(crate) fn ok_of_tuple() -> ValueAndType {
        let tuple_str = test_utils::convert_value_and_type_to_str(&tuple());
        let wave_str = format!("ok({tuple_str})");
        test_utils::get_value_and_type(&data_types::result_of_tuple_type(), wave_str.as_str())
    }

    pub(crate) fn err_of_tuple() -> ValueAndType {
        let tuple_str = test_utils::convert_value_and_type_to_str(&tuple());
        let wave_str = format!("err({tuple_str})");
        test_utils::get_value_and_type(&data_types::result_of_tuple_type(), wave_str.as_str())
    }

    pub(crate) fn ok_of_flag() -> ValueAndType {
        test_utils::get_value_and_type(&data_types::result_of_flag_type(), "ok({featurex})")
    }

    pub(crate) fn err_of_flag() -> ValueAndType {
        test_utils::get_value_and_type(&data_types::result_of_flag_type(), "err({featurex})")
    }

    pub(crate) fn ok_of_record() -> ValueAndType {
        let record_str = test_utils::convert_value_and_type_to_str(&record());
        let wave_str = format!("ok({})", &record_str);
        test_utils::get_value_and_type(&data_types::result_of_record_type(), wave_str.as_str())
    }

    pub(crate) fn err_of_record() -> ValueAndType {
        let record_str = test_utils::convert_value_and_type_to_str(&record());
        let wave_str = format!("err({})", &record_str);
        test_utils::get_value_and_type(&data_types::result_of_record_type(), wave_str.as_str())
    }

    pub(crate) fn ok_of_list() -> ValueAndType {
        test_utils::get_value_and_type(&data_types::result_of_list_type(), "ok([\"foo\"])")
    }

    pub(crate) fn err_of_list() -> ValueAndType {
        test_utils::get_value_and_type(&data_types::result_of_list_type(), "err([\"foo\"])")
    }

    pub(crate) fn list_of_number() -> ValueAndType {
        test_utils::get_value_and_type(&data_types::list_of_number_type_type(), "[42]")
    }

    pub(crate) fn list_of_str() -> ValueAndType {
        test_utils::get_value_and_type(&data_types::list_of_str_type(), "[\"foo\"]")
    }

    pub(crate) fn list_of_option() -> ValueAndType {
        test_utils::get_value_and_type(&data_types::list_of_option_type(), "[some(\"foo\")]")
    }

    pub(crate) fn list_of_list() -> ValueAndType {
        test_utils::get_value_and_type(&data_types::list_of_list_type(), "[[\"foo\"]]")
    }

    pub(crate) fn list_of_variant() -> ValueAndType {
        test_utils::get_value_and_type(&data_types::list_of_variant_type(), "[case-str(\"foo\")]")
    }

    pub(crate) fn list_of_enum() -> ValueAndType {
        test_utils::get_value_and_type(&data_types::list_of_enum_type(), "[enum-a]")
    }

    pub(crate) fn list_of_tuple() -> ValueAndType {
        let tuple_str = test_utils::convert_value_and_type_to_str(&tuple());
        let wave_str = format!("[{}, {}]", &tuple_str, &tuple_str);
        test_utils::get_value_and_type(&data_types::list_of_tuple(), wave_str.as_str())
    }

    pub(crate) fn list_of_record() -> ValueAndType {
        let record_str = test_utils::convert_value_and_type_to_str(&record());
        let wave_str = format!("[{}]", &record_str);
        test_utils::get_value_and_type(&data_types::list_of_record_type(), wave_str.as_str())
    }

    pub(crate) fn some_of_number() -> ValueAndType {
        test_utils::get_value_and_type(&data_types::option_of_number_type(), "some(42)")
    }

    pub(crate) fn none_of_number() -> ValueAndType {
        test_utils::get_value_and_type(&data_types::option_of_number_type(), "none")
    }

    pub(crate) fn some_of_str() -> ValueAndType {
        test_utils::get_value_and_type(&data_types::option_of_str_type(), "some(\"foo\")")
    }

    pub(crate) fn none_of_str() -> ValueAndType {
        test_utils::get_value_and_type(&data_types::option_of_str_type(), "none")
    }

    pub(crate) fn some_of_some() -> ValueAndType {
        test_utils::get_value_and_type(&data_types::option_of_option_type(), "some(some(\"foo\"))")
    }

    pub(crate) fn none_of_some() -> ValueAndType {
        test_utils::get_value_and_type(&data_types::option_of_option_type(), "none")
    }

    pub(crate) fn some_of_variant() -> ValueAndType {
        test_utils::get_value_and_type(
            &data_types::option_of_variant_type(),
            "some(case-str(\"foo\"))",
        )
    }

    pub(crate) fn none_of_variant() -> ValueAndType {
        test_utils::get_value_and_type(&data_types::option_of_variant_type(), "none")
    }

    pub(crate) fn some_of_enum() -> ValueAndType {
        test_utils::get_value_and_type(&data_types::option_of_enum_type(), "some(enum-a)")
    }

    pub(crate) fn none_of_enum() -> ValueAndType {
        test_utils::get_value_and_type(&data_types::option_of_enum_type(), "none")
    }

    pub(crate) fn some_of_tuple() -> ValueAndType {
        let tuple_str = test_utils::convert_value_and_type_to_str(&tuple());
        let wave_str = format!("some({tuple_str})");
        test_utils::get_value_and_type(&data_types::option_of_tuple(), wave_str.as_str())
    }

    pub(crate) fn none_of_tuple() -> ValueAndType {
        test_utils::get_value_and_type(&data_types::option_of_tuple(), "none")
    }

    pub(crate) fn some_of_record() -> ValueAndType {
        let record_str = test_utils::convert_value_and_type_to_str(&record());
        let wave_str = format!("some({})", &record_str);
        test_utils::get_value_and_type(&data_types::option_of_record_type(), wave_str.as_str())
    }

    pub(crate) fn none_of_record() -> ValueAndType {
        test_utils::get_value_and_type(&data_types::option_of_record_type(), "none")
    }

    pub(crate) fn some_of_list() -> ValueAndType {
        test_utils::get_value_and_type(&data_types::option_of_list(), "some([\"foo\"])")
    }

    pub(crate) fn none_of_list() -> ValueAndType {
        test_utils::get_value_and_type(&data_types::option_of_list(), "none")
    }

    pub(crate) fn tuple() -> ValueAndType {
        test_utils::get_value_and_type(
            &data_types::tuple_type(),
            r#"
          ("foo", 42, 42, 42, 42, true, 'a', some(42), ok(42), [true], case-hello(42.0), {field-one: true, field-two: "foo"})"#,
        )
    }

    pub(crate) fn enum_data() -> ValueAndType {
        test_utils::get_value_and_type(&data_types::enum_type(), "enum-a")
    }

    pub(crate) fn str_data() -> ValueAndType {
        test_utils::get_value_and_type(&data_types::str_type(), "\"foo\"")
    }

    pub(crate) fn number_data() -> ValueAndType {
        test_utils::get_value_and_type(&data_types::number_type(), "42")
    }

    pub(crate) fn flag() -> ValueAndType {
        test_utils::get_value_and_type(&data_types::flag_type(), "{featurex}")
    }

    pub(crate) fn variant() -> ValueAndType {
        test_utils::get_value_and_type(&data_types::variant_type(), "case-str(\"foo\")")
    }

    pub(crate) fn record() -> ValueAndType {
        test_utils::get_value_and_type(
            &data_types::record_type(),
            r#"
          {
            string-headers: {authorization-string: "foo"},
            data-body: {
              str: "foo",
              list-of-str: ["foo"],
              list-of-option: ["foo"],
              list-of-list: [["foo"]],
              list-of-variant: [case-str("foo")],
              list-of-enum: [enum-a],
              list-of-tuple: [("foo", 42, 42, 42, 42, true, 'a', some(42), ok(42), [true], case-hello(42.0), {field-one: true, field-two: "foo"})],
              list-of-record: [{field-string-one: "foo", field-string-two: "foo"}],
              nested-record: {field-string-one: "foo", field-string-two: "foo"},
              option-of-str: some("foo"),
              option-of-option: some(some("foo")),
              option-of-variant: some(case-str("foo")),
              option-of-enum: some(enum-a),
              option-of-tuple: some(("foo", 42, 42, 42, 42, true, 'a', some(42), ok(42), [true], case-hello(42.0), {field-one: true, field-two: "foo"})),
              option-of-record: some({field-string-one: "foo", field-string-two: "foo"}),
              option-of-list: some(["foo"]),
              variant-data-a: case-str("foo")
              variant-data-b: case-str("foo"),
              variant-data-c: case-str("foo"),
              variant-data-d: case-str("foo"),
              variant-data-e: case-str("foo"),
              variant-data-f: case-str("foo"),
              variant-data-g: case-str("foo"),
              enum-data-a: enum-a,
              enum-data-b: enum-b,
              enum-data-c: enum-c,
              flags-data-a: { featurex },
              flags-data-b: { featurex, featurey },
              flags-data-c: { featurex, featurey, featurez },
              result-data-a: ok("foo"),
              result-data-b: ok(42),
              result-data-c: ok(enum-a),
              result-data-d: ok(case-str("foo")),
              result-data-e: ok(("foo", 42, 42, 42, 42, true, 'a', some(42), ok(42), [true], case-hello(42.0), {field-one: true, field-two: "foo"})),
              result-data-f: ok(some("foo")),
              result-data-g: err("foo"),
              result-data-h: err(42),
              result-data-i: err(enum-a),
              result-data-j: err(case-str("foo")),
              result-data-k: err(("foo", 42, 42, 42, 42, true, 'a', some(42), ok(42), [true], case-hello(42.0), {field-one: true, field-two: "foo"})),
              result-data-l: err(some("foo")),
              result-data-m: ok({ featurex, featurey, featurez }),
              result-data-n: err({ featurex, featurey, featurez })
              tuple-data: ("foo", 42, 42, 42, 42, true, 'a', some(42), ok(42), [true], case-hello(42.0), {field-one: true, field-two: "foo"}),
              character-data : 'x',
              f64-data : 3.14,
              f32-data : 3.14,
              u64-data : 42,
              s64-data : 42,
              u32-data : 42,
              s32-data : 42,
              u16-data : 42,
              s16-data : 42,
              u8-data : 42,
              s8-data : 42,
              boolean-data : true
           }
          }"#,
        )
    }
}

mod mock_interpreter {
    use crate::{mock_data, test_utils, Interpreter};
    use crate::{
        EvaluatedFnArgs, EvaluatedFqFn, EvaluatedWorkerName, RibComponentFunctionInvoke,
        RibFunctionInvokeResult, RibInput,
    };
    use async_trait::async_trait;

    use golem_wasm::analysis::analysed_type::{field, record, str};
    use golem_wasm::analysis::AnalysedType;
    use golem_wasm::ValueAndType;
    use rib::{ComponentDependencyKey, DefaultWorkerNameGenerator, InstructionId};
    use std::collections::HashMap;
    use std::sync::Arc;

    pub(crate) fn interpreter() -> Interpreter {
        let functions_and_results: Vec<(&str, Option<ValueAndType>)> = vec![
            ("function-unit-response", None),
            ("function-no-arg", Some(mock_data::str_data())),
            ("function-no-arg-unit", None),
            ("function-str-response", Some(mock_data::str_data())),
            ("function-number-response", Some(mock_data::number_data())),
            ("function-some-str-response", Some(mock_data::some_of_str())),
            ("function-none-str-response", Some(mock_data::none_of_str())),
            (
                "function-some-number-response",
                Some(mock_data::some_of_number()),
            ),
            (
                "function-none-number-response",
                Some(mock_data::none_of_number()),
            ),
            (
                "function-some-option-response",
                Some(mock_data::some_of_some()),
            ),
            (
                "function-none-option-response",
                Some(mock_data::none_of_some()),
            ),
            (
                "function-some-variant-response",
                Some(mock_data::some_of_variant()),
            ),
            (
                "function-none-variant-response",
                Some(mock_data::none_of_variant()),
            ),
            (
                "function-some-enum-response",
                Some(mock_data::some_of_enum()),
            ),
            (
                "function-none-enum-response",
                Some(mock_data::none_of_enum()),
            ),
            (
                "function-some-tuple-response",
                Some(mock_data::some_of_tuple()),
            ),
            (
                "function-none-tuple-response",
                Some(mock_data::none_of_tuple()),
            ),
            (
                "function-some-record-response",
                Some(mock_data::some_of_record()),
            ),
            (
                "function-none-record-response",
                Some(mock_data::none_of_record()),
            ),
            (
                "function-some-list-response",
                Some(mock_data::some_of_list()),
            ),
            (
                "function-none-list-response",
                Some(mock_data::none_of_list()),
            ),
            (
                "function-list-number-response",
                Some(mock_data::list_of_number()),
            ),
            ("function-list-str-response", Some(mock_data::list_of_str())),
            (
                "function-list-option-response",
                Some(mock_data::list_of_option()),
            ),
            (
                "function-list-list-response",
                Some(mock_data::list_of_list()),
            ),
            (
                "function-list-variant-response",
                Some(mock_data::list_of_variant()),
            ),
            (
                "function-list-enum-response",
                Some(mock_data::list_of_enum()),
            ),
            (
                "function-list-tuple-response",
                Some(mock_data::list_of_tuple()),
            ),
            (
                "function-list-record-response",
                Some(mock_data::list_of_record()),
            ),
            ("function-ok-str-response", Some(mock_data::ok_of_str())),
            ("function-err-str-response", Some(mock_data::err_of_str())),
            (
                "function-ok-number-response",
                Some(mock_data::ok_of_number()),
            ),
            (
                "function-err-number-response",
                Some(mock_data::err_of_number()),
            ),
            (
                "function-ok-option-response",
                Some(mock_data::ok_of_option()),
            ),
            (
                "function-err-option-response",
                Some(mock_data::err_of_option()),
            ),
            (
                "function-ok-variant-response",
                Some(mock_data::ok_of_variant()),
            ),
            (
                "function-err-variant-response",
                Some(mock_data::err_of_variant()),
            ),
            ("function-ok-enum-response", Some(mock_data::ok_of_enum())),
            ("function-err-enum-response", Some(mock_data::err_of_enum())),
            ("function-ok-tuple-response", Some(mock_data::ok_of_tuple())),
            (
                "function-err-tuple-response",
                Some(mock_data::err_of_tuple()),
            ),
            ("function-ok-flag-response", Some(mock_data::ok_of_flag())),
            ("function-err-flag-response", Some(mock_data::err_of_flag())),
            (
                "function-ok-record-response",
                Some(mock_data::ok_of_record()),
            ),
            (
                "function-err-record-response",
                Some(mock_data::err_of_record()),
            ),
            ("function-ok-list-response", Some(mock_data::ok_of_list())),
            ("function-err-list-response", Some(mock_data::err_of_list())),
            ("function-tuple-response", Some(mock_data::tuple())),
            ("function-enum-response", Some(mock_data::enum_data())),
            ("function-flag-response", Some(mock_data::flag())),
            ("function-variant-response", Some(mock_data::variant())),
            ("function-record-response", Some(mock_data::record())),
            ("function-all-inputs", Some(mock_data::str_data())),
        ];

        let functions_and_result: HashMap<FunctionName, Option<ValueAndType>> =
            functions_and_results
                .into_iter()
                .map(|(name, result)| (FunctionName(name.to_string()), result))
                .collect();

        let record_input_type = record(vec![
            field("headers", record(vec![field("name", str())])),
            field("body", record(vec![field("name", str())])),
            field("path", record(vec![field("name", str())])),
        ]);

        let record_input_value = test_utils::get_value_and_type(
            &record_input_type,
            r#" { headers : { name : "foo" }, body : { name : "bar" }, path : { name : "baz" } }"#,
        );

        let mut interpreter_env_input: HashMap<String, ValueAndType> = HashMap::new();
        interpreter_env_input.insert("request".to_string(), record_input_value);

        dynamic_test_interpreter(functions_and_result, interpreter_env_input)
    }

    #[derive(Clone, Hash, PartialEq, Eq)]
    struct FunctionName(pub(crate) String);

    fn dynamic_test_interpreter(
        functions_and_result: HashMap<FunctionName, Option<ValueAndType>>,
        interpreter_env_input: HashMap<String, ValueAndType>,
    ) -> Interpreter {
        let dynamic_worker_invoke = Arc::new(DynamicRibFunctionInvoke {
            functions_and_result,
        });

        Interpreter::new(
            RibInput::new(interpreter_env_input),
            dynamic_worker_invoke,
            Arc::new(DefaultWorkerNameGenerator),
        )
    }

    struct DynamicRibFunctionInvoke {
        functions_and_result: HashMap<FunctionName, Option<ValueAndType>>,
    }

    #[async_trait]
    impl RibComponentFunctionInvoke for DynamicRibFunctionInvoke {
        async fn invoke(
            &self,
            _component_info: ComponentDependencyKey,
            _instruction_id: &InstructionId,
            _worker_name: EvaluatedWorkerName,
            function_name: EvaluatedFqFn,
            _args: EvaluatedFnArgs,
            _return_type: Option<AnalysedType>,
        ) -> RibFunctionInvokeResult {
            let function_name = FunctionName(function_name.0);

            let result = self
                .functions_and_result
                .get(&function_name)
                .cloned()
                .flatten();

            Ok(result)
        }
    }
}

mod test_utils {
    use golem_wasm::analysis::*;
    use golem_wasm::ValueAndType;

    pub(crate) fn get_value_and_type(
        analysed_type: &AnalysedType,
        wasm_wave_str: &str,
    ) -> ValueAndType {
        let result = golem_wasm::parse_value_and_type(analysed_type, wasm_wave_str);

        match result {
            Ok(value) => value,
            Err(err) => panic!("Wasm wave syntax error {analysed_type:?} {wasm_wave_str} {err}"),
        }
    }

    pub(crate) fn convert_value_and_type_to_str(value: &ValueAndType) -> String {
        golem_wasm::print_value_and_type(value).unwrap()
    }

    pub(crate) fn get_function_component_metadata(
        function_name: &str,
        input_types: Vec<AnalysedType>,
        output: Option<AnalysedType>,
    ) -> Vec<AnalysedExport> {
        let analysed_function_parameters = input_types
            .into_iter()
            .enumerate()
            .map(|(index, typ)| AnalysedFunctionParameter {
                name: format!("param{index}"),
                typ,
            })
            .collect();

        let result = output.map(|typ| AnalysedFunctionResult { typ });

        vec![AnalysedExport::Function(AnalysedFunction {
            name: function_name.to_string(),
            parameters: analysed_function_parameters,
            result,
        })]
    }
}
