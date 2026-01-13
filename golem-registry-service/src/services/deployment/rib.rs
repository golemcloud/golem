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

// use golem_service_base::custom_api::HttpCors;
// use rib::{Expr, GetLiteralValue, RibCompiler, RibInput};

// pub struct CorsPreflightExpr(pub Expr);

// impl CorsPreflightExpr {
//     pub fn into_http_cors(self) -> Result<HttpCors, String> {
//         let rib_compiler = RibCompiler::default();
//         let compiled_expr = rib_compiler
//             .compile(self.0)
//             .map_err(|err| format!("Rib compilation for cors-preflight response. {err}"))?;

//         let rib_input = RibInput::default();
//         let evaluate_rib = rib::interpret_pure(compiled_expr.byte_code, rib_input, None);

//         let result = futures::executor::block_on(evaluate_rib).map_err(|err| {
//             format!("Failed to evaluate Rib script to form pre-flight CORS {err}")
//         })?;

//         let record = result
//             .get_record()
//             .ok_or("Invalid pre-flight CORS response mapping")?;

//         let mut cors = HttpCors::default();

//         for (key, value) in record {
//             let value = value
//                 .get_literal()
//                 .ok_or(format!(
//                     "Invalid value for key {key} in CORS preflight response"
//                 ))?
//                 .as_string();

//             Self::set_cors_field(&mut cors, &key, value)?;
//         }

//         Ok(cors)
//     }

//     fn set_cors_field(cors: &mut HttpCors, key: &str, value: String) -> Result<(), String> {
//         match key.to_lowercase().as_str() {
//             "access-control-allow-origin" => {
//                 cors.allow_origin = value;
//             }
//             "access-control-allow-methods" => {
//                 cors.allow_methods = value;
//             }
//             "access-control-allow-headers" => {
//                 cors.allow_headers = value;
//             }
//             "access-control-expose-headers" => {
//                 cors.expose_headers = Some(value);
//             }
//             "access-control-allow-credentials" => {
//                 let allow = value
//                     .parse::<bool>()
//                     .map_err(|_| "Invalid value for max age".to_string())?;

//                 cors.allow_credentials = Some(allow);
//             }
//             "access-control-max-age" => {
//                 let max_age = value
//                     .parse::<u64>()
//                     .map_err(|_| "Invalid value for max age".to_string())?;

//                 cors.max_age = Some(max_age);
//             }
//             _ => {
//                 return Err("Invalid cors header in the rib for pre-flight. Allowed keys: access-control-allow-origin, access-control-allow-methods, access-control-allow-headers, access-control-expose-headers, and access-control-max-age".to_string());
//             }
//         }
//         Ok(())
//     }
// }
