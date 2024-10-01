mod mock_interpreter;
mod function_metadata;
mod component_metadata;
mod test_utils;
mod data_types;
mod mock_data;

#[cfg(test)]
mod complex_test {
    use golem_wasm_ast::analysis::{AnalysedType, TypeStr, TypeU64, TypeU8};
    use crate::{compiler, Expr};
    use crate::interpreter::comprehensive_test::{component_metadata, internal, mock_interpreter, test_utils};

    #[tokio::test]
    async fn test_interpreter_complex_rib() {
        let expr = r#"

              let str1 = request.body.name;
              let str2 = request.headers.name;
              let str3 = requuest.path.name;



              let input = { body : { id: "bId", name: "bName", titles: request.body.titles, address: request.body.address } };
              let optional_result = function-option(input);
              let id: str = request.body.id;

              let a = match optional_result {
                some(value) => "personal-id",
                none => id
               };

              let b = match optional_result {
                some(value) => "personal-id",
                none =>  input.body.titles[1]
               };

              let c = match optional_result {
                some(value) => "personal-id",
                none =>  request.body.address.street
              };

              let authorisation: str = request.headers.authorisation;
              let success: u64 = 200;
              let failure: u64 = 401;

              let d = if authorisation == "admin" then success else failure;

              function-no-arg-unit();

              let unit_result = function-unit(input);

              let ok_result = function-ok(input);

              let e = if id == "foo" then "bar" else match ok_result {
                ok(value) => value,
                err(msg) => "empty"
               };

              let err_result = function-err(input);

              let f = if id == "foo" then "bar" else match err_result { ok(value) => value, err(msg) => msg };

              let ok_record_result = function-ok-record(input);

              let g = match ok_record_result {
                ok(success_rec) => success_rec.b[0],
                err(failure_rec) => "${failure_rec.d}"
              };

              let err_record_result = function-err-record(input);

              let h = match err_record_result { ok(success_rec) => success_rec.b[0], err(failure_rec) => "${failure_rec.d}" };
              let i = match err_record_result { ok(_) => 1u8, err(_) => 0u8 };
              let j = match ok_record_result { ok(_) => 1u8, err(_) => 0u8 };

              let variant_result = function-variant(input);

              let k = match variant_result {
                 process-user(value) => value,
                 register-user(value) => "${value}",
                 validate => "validated"
              };

              let processed = process-user("jon");
              let registered = register-user(1);

              let m = match processed {
                process-user(name) => name,
                register-user(value) => "${value}",
                validate => "validated"
              };

              let n = match registered {
                process-user(name) => name,
                register-user(value) => "${value}",
                validate => "validated"
              };

              { a : a, b : b, c: c, d: d, e: e, f: f, g: g, h: h, i: i, j: j, k: k, m: m, n: n}
        "#;

        let expr = Expr::from_text(expr).unwrap();

        let compiled_expr = compiler::compile(&expr, &component_metadata::component_metadata()).unwrap().byte_code;

        let mut rib_executor = mock_interpreter::interpreter();
        let result = rib_executor.run(compiled_expr).await.unwrap();

        let expected_result = test_utils::get_type_annotated_value(
            &test_utils::analysed_type_record(vec![
                ("a", AnalysedType::Str(TypeStr)),
                ("b", AnalysedType::Str(TypeStr)),
                ("c", AnalysedType::Str(TypeStr)),
                ("d", AnalysedType::U64(TypeU64)),
                ("e", AnalysedType::Str(TypeStr)),
                ("f", AnalysedType::Str(TypeStr)),
                ("g", AnalysedType::Str(TypeStr)),
                ("h", AnalysedType::Str(TypeStr)),
                ("i", AnalysedType::U8(TypeU8)),
                ("j", AnalysedType::U8(TypeU8)),
                ("k", AnalysedType::Str(TypeStr)),
                ("m", AnalysedType::Str(TypeStr)),
                ("n", AnalysedType::Str(TypeStr)),
            ]),
            r#" { a : "bId", b : "bTitle2", c : "bStreet", d: 200, e: "success", f: "failure", g: "bar", h : "fuuz", i: 0, j: 1, k: "validated", m:"jon", n: "1" }"#,
        );
        assert_eq!(result.get_val().unwrap(), expected_result);
    }
}


