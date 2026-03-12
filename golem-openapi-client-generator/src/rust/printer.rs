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

use crate::printer::{IndentContext, NewLine, PrintContext, Printer, TreePrinter};
use itertools::Itertools;
use std::collections::HashSet;
use std::fmt::Display;
use std::ops::Add;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RustUse(String);

pub struct RustContext {
    imports: HashSet<RustUse>,
    code: String,
    depth: usize,
}

impl RustContext {
    pub fn new() -> RustContext {
        RustContext {
            imports: HashSet::new(),
            code: String::new(),
            depth: 0,
        }
    }

    pub fn print_to_string(mut self, p: TreePrinter<RustContext>) -> String {
        p.print(&mut self);

        self.to_string()
    }
}

impl Display for RustContext {
    #[allow(unstable_name_collisions)]
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let RustContext {
            imports,
            code,
            depth: _,
        } = self;

        if imports.is_empty() {
            write!(f, "{}", code)
        } else {
            let imports: String = imports
                .iter()
                .map(|i| format!("use {};", i.0))
                .sorted()
                .intersperse("\n".to_string())
                .collect();

            write!(f, "{imports}\n\n{code}")
        }
    }
}

impl PrintContext for RustContext {
    fn print_str(&mut self, s: &str) {
        self.code.push_str(s)
    }
}

impl IndentContext for RustContext {
    fn get_depth(&self) -> usize {
        self.depth
    }

    fn increment_ident(&mut self) {
        self.depth += 1;
    }

    fn decrement_ident(&mut self) {
        self.depth -= 1;
    }
}

pub struct RustCode {
    imports: HashSet<RustUse>,
    code: String,
}

impl Printer<RustContext> for RustCode {
    fn print(&self, ctx: &mut RustContext) {
        ctx.imports.extend(self.imports.clone());
        ctx.code.push_str(&self.code);
    }
}

impl Add<RustCode> for TreePrinter<RustContext> {
    type Output = TreePrinter<RustContext>;

    fn add(self, rhs: RustCode) -> Self::Output {
        self.node(rhs)
    }
}

pub fn indent() -> TreePrinter<RustContext> {
    TreePrinter::indent()
}

pub trait IntoRustTree {
    fn tree(self) -> TreePrinter<RustContext>;
}

pub fn unit() -> TreePrinter<RustContext> {
    TreePrinter::unit()
}

pub fn line<T: IntoRustTree>(code: T) -> TreePrinter<RustContext> {
    indent() + code.tree() + NewLine
}

pub fn rust_name(import: &str, name: &str) -> TreePrinter<RustContext> {
    let import_name = if name.ends_with('!') {
        &name[0..name.len() - 1]
    } else {
        name
    };
    TreePrinter::leaf(RustCode {
        imports: HashSet::from([RustUse(format!("{import}::{import_name}"))]),
        code: name.to_string(),
    })
}

pub fn rust_name_with_alias(import: &str, name: &str, alias: &str) -> TreePrinter<RustContext> {
    let import_name = if name.ends_with('!') {
        &name[0..name.len() - 1]
    } else {
        name
    };
    TreePrinter::leaf(RustCode {
        imports: HashSet::from([RustUse(format!("{import}::{import_name} as {alias}"))]),
        code: alias.to_string(),
    })
}

impl IntoRustTree for TreePrinter<RustContext> {
    fn tree(self) -> TreePrinter<RustContext> {
        self
    }
}

impl<T: Printer<RustContext> + 'static> IntoRustTree for T {
    fn tree(self) -> TreePrinter<RustContext> {
        TreePrinter::leaf(self)
    }
}

#[cfg(test)]
mod tests {
    use indoc::indoc;

    use crate::printer::indented;
    use crate::rust::printer::{line, rust_name, unit, RustContext, RustUse};
    use std::collections::HashSet;

    #[test]
    fn indent_and_imports() {
        let info = rust_name("trace", "info!");
        let error = rust_name("trace", "error!");

        #[rustfmt::skip]
            let p = unit()
            + line("pub fn m() {")
            + indented(
            line(info + "(\"abc\");")
                + line(error + "(\"def\")"))
            + line("}");

        let mut ctx = RustContext::new();

        p.print(&mut ctx);

        let expected = indoc! {r#"
            pub fn m() {
                info!("abc");
                error!("def")
            }
        "# };

        let expected_res = indoc! {r#"
            use trace::error;
            use trace::info;

            pub fn m() {
                info!("abc");
                error!("def")
            }
        "# };

        assert_eq!(
            ctx.imports,
            HashSet::from([
                RustUse("trace::info".to_string()),
                RustUse("trace::error".to_string())
            ])
        );
        assert_eq!(ctx.depth, 0);
        assert_eq!(ctx.code, expected);
        assert_eq!(ctx.to_string(), expected_res);
    }
}
