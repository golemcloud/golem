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
use crate::rust::printer::*;
use crate::rust::types::{escape_keywords, RustPrinter};
use convert_case::{Case, Casing};
use itertools::Itertools;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Ord, PartialOrd)]
pub enum Verbosity {
    Default,
    Pub,
}

impl Verbosity {
    fn render(&self) -> RustPrinter {
        match self {
            Verbosity::Default => unit(),
            Verbosity::Pub => unit() + "pub ",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Ord, PartialOrd)]
pub struct ModuleName {
    name: String,
    verbosity: Verbosity,
}

impl ModuleName {
    pub fn name(&self) -> String {
        self.name.clone()
    }

    fn code(&self) -> RustPrinter {
        line(unit() + self.verbosity.render() + "mod " + escape_keywords(&self.name) + ";")
    }

    pub fn new(s: impl AsRef<str>) -> ModuleName {
        ModuleName {
            name: Self::escape_type_params(s.as_ref()).to_case(Case::Snake),
            verbosity: Verbosity::Default,
        }
    }

    pub fn new_pub(s: impl AsRef<str>) -> ModuleName {
        ModuleName {
            name: Self::escape_type_params(s.as_ref()).to_case(Case::Snake),
            verbosity: Verbosity::Pub,
        }
    }

    pub fn file_name(&self) -> String {
        format!("{}.rs", &self.name)
    }

    fn escape_type_params(s: &str) -> String {
        s.replace("<", "_")
            .replace(",", "_")
            .replace(">", "_")
            .replace(" ", "")
    }
}

#[derive(Debug, Clone)]
pub struct ModuleDef {
    pub name: ModuleName,
    pub exports: Vec<String>,
}

pub struct Module {
    pub def: ModuleDef,
    pub code: String,
}

impl ModuleDef {
    #[rustfmt::skip]
    fn render_exports(&self, parent: &str) -> RustPrinter {
        self.exports
            .iter()
            .sorted()
            .map(|e| line(unit() + "pub use " + parent + "::" + escape_keywords(&self.name.name) + "::" + e + ";"))
            .reduce(|acc, e| acc + e)
            .unwrap_or_else(unit)
    }

    pub fn new(name: ModuleName) -> ModuleDef {
        ModuleDef {
            name,
            exports: Vec::new(),
        }
    }
}

pub fn lib_gen(self_name: &str, modules: &[ModuleDef], disable_clippy: bool) -> String {
    let mods = modules
        .iter()
        .map(|m| &m.name)
        .sorted()
        .map(|i| i.code())
        .reduce(|acc, e| acc + e)
        .unwrap_or_else(unit);

    let uses = modules
        .iter()
        .sorted_by_key(|m| &m.name)
        .map(|d| d.render_exports(self_name))
        .reduce(|acc, e| acc + e)
        .unwrap_or_else(unit);

    let base = if disable_clippy {
        line(unit() + "#[allow(clippy::all)]")
    } else {
        unit()
    };
    let code = base + NewLine + mods + NewLine + uses;

    RustContext::new().print_to_string(code)
}

#[cfg(test)]
mod tests {
    use indoc::indoc;

    use crate::rust::lib_gen::{lib_gen, ModuleDef, ModuleName};

    #[test]
    fn simple_lib() {
        let res = lib_gen(
            "lib",
            &[
                ModuleDef {
                    name: ModuleName::new("abc"),
                    exports: vec!["C".to_string(), "B".to_string()],
                },
                ModuleDef {
                    name: ModuleName::new("xyz"),
                    exports: vec!["A".to_string(), "Y".to_string()],
                },
            ],
            true,
        );

        let expected = indoc! { r#"
            #[allow(clippy::all)]

            mod abc;
            mod xyz;

            pub use lib::abc::B;
            pub use lib::abc::C;
            pub use lib::xyz::A;
            pub use lib::xyz::Y;
        "#};

        assert_eq!(res, expected)
    }
}
