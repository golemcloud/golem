// Copyright 2024 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::printer::*;
use crate::rust::lib_gen::{Module, ModuleDef, ModuleName};
use crate::rust::printer::*;
use crate::{Error, Result};
use convert_case::{Case, Casing};
use openapiv3::{OpenAPI, ReferenceOr, SecurityScheme};

fn client() -> TreePrinter<RustContext> {
    rust_name("reqwest", "Client")
}

fn url() -> TreePrinter<RustContext> {
    rust_name("reqwest", "Url")
}

struct Security {
    typedef: TreePrinter<RustContext>,
    filed: TreePrinter<RustContext>,
    methods: TreePrinter<RustContext>,
}

fn security_gen(name: &str, ref_or_sec: &ReferenceOr<SecurityScheme>) -> Result<Security> {
    match ref_or_sec {
        ReferenceOr::Reference { .. } => Err(Error::unimplemented(format!(
            "$ref in security_schemes[{name}]"
        ))),
        ReferenceOr::Item(sec) => {
            match sec {
                SecurityScheme::HTTP { scheme, .. } => {
                    if scheme == "bearer" {
                        #[rustfmt::skip]
                        let typedef = unit() +
                            line("#[derive(Debug, Clone)]") +
                            line("pub enum Security {") +
                            indented(
                                line("Empty,") +
                                line("Bearer(String),")
                            ) +
                            line("}") +
                            NewLine +
                            line("impl Security {") +
                            indented(
                               line("pub fn bearer<S: Into<String>>(s: S) -> Security { Security::Bearer(s.into()) }")
                            ) +
                            line("}");

                        let field_name = format!(
                            "security_{}",
                            name.from_case(Case::UpperCamel).to_case(Case::Snake)
                        );

                        let filed = line(unit() + "pub " + &field_name + ": Security,");

                        #[rustfmt::skip]
                        let methods = unit() +
                            line("pub fn bearer_token(&self) -> Option<&str> {") +
                              indented(
                                line(unit() + "match &self." + &field_name + "{") +
                                  indented(
                                    line("Security::Empty => None,") +
                                    line(r#"Security::Bearer(token) => Some(token),"#)
                                  ) +
                                  line("}")
                              ) +
                            line("}");

                        Ok(Security {
                            typedef,
                            filed,
                            methods,
                        })
                    } else {
                        Err(Error::unimplemented(format!("Unsupported http security_schemes[{name}], only bearer is implemented.")))
                    }
                }
                _ => Err(Error::unimplemented(format!(
                    "Unsupported security_schemes[{name}], only http is implemented."
                ))),
            }
        }
    }
}

pub fn context_gen(open_api: &OpenAPI) -> Result<Module> {
    let security = match &open_api.components {
        None => None,
        Some(components) => {
            let ref_sec = components
                .security_schemes
                .iter()
                .find_map(|(name, ref_or)| {
                    if ref_or.as_item().is_none() {
                        Some(name)
                    } else {
                        None
                    }
                });
            if let Some(name) = ref_sec {
                return Err(Error::unimplemented(format!(
                    "$ref in security_schemes[{name}]"
                )));
            }

            let sec_res: Result<Vec<Security>> = components
                .security_schemes
                .iter()
                // Remove cookie security scheme.
                .filter(|(_, sec)| {
                    !matches!(
                        sec,
                        ReferenceOr::Item(SecurityScheme::APIKey {
                            location: openapiv3::APIKeyLocation::Cookie,
                            ..
                        })
                    )
                })
                .map(|(name, sec)| security_gen(name, sec))
                .collect();

            sec_res?.pop()
        }
    };

    let no_security = security.is_none();

    let security = security.unwrap_or(Security {
        typedef: unit(),
        filed: unit(),
        methods: line("pub fn bearer_token(&self) -> Option<&str> { None }"),
    });

    #[rustfmt::skip]
    let code = unit() +
        security.typedef +
        NewLine +
        line("#[derive(Debug, Clone)]") +
        line(unit() + "pub struct Context {") +
        indented(
            line(unit() + "pub client: " + client() + ",") +
                line(unit() + "pub base_url: " + url() + ",") +
                security.filed
        ) +
        line("}") +
        NewLine +
        line("impl Context {") +
        indented(
            security.methods
        ) +
        line("}");

    let mut exports = vec!["Context".to_string()];

    if !no_security {
        exports.push("Security".to_string());
    }

    Ok(Module {
        def: ModuleDef {
            name: ModuleName::new("context"),
            exports,
        },
        code: RustContext::new().print_to_string(code),
    })
}
