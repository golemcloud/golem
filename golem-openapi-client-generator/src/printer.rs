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

use std::ops::Add;

#[derive(Clone, Debug)]
#[allow(dead_code)]
pub struct Error;

pub trait Printer<Context> {
    fn print(&self, ctx: &mut Context);
}

pub trait IndentContext {
    fn get_depth(&self) -> usize;
    fn increment_ident(&mut self);
    fn decrement_ident(&mut self);
    fn indent_width(&self) -> usize {
        4
    }
    fn indent_char(&self) -> char {
        ' '
    }
    fn print_indent(&mut self)
    where
        Self: PrintContext,
    {
        self.print_str(
            &self
                .indent_char()
                .to_string()
                .repeat(self.get_depth() * self.indent_width()),
        )
    }
}

pub trait PrintContext {
    fn print_str(&mut self, s: &str);

    fn new_line(&self) -> char {
        '\n'
    }

    fn print_new_line(&mut self) {
        self.print_str(&self.new_line().to_string())
    }
}

impl<Context: PrintContext> Printer<Context> for &str {
    fn print(&self, ctx: &mut Context) {
        ctx.print_str(self)
    }
}

impl<Context: PrintContext> Printer<Context> for &String {
    fn print(&self, ctx: &mut Context) {
        ctx.print_str(self)
    }
}

impl<Context: PrintContext> Printer<Context> for String {
    fn print(&self, ctx: &mut Context) {
        ctx.print_str(self)
    }
}

pub struct IndentPrinter;

impl<C: IndentContext + PrintContext> Printer<C> for IndentPrinter {
    fn print(&self, ctx: &mut C) {
        ctx.print_indent()
    }
}

pub enum TreePrinter<Context> {
    Leaf(Box<dyn Printer<Context>>),
    Node {
        left: Box<TreePrinter<Context>>,
        right: Box<TreePrinter<Context>>,
    },
    Unit,
}

impl<Context> TreePrinter<Context> {
    pub fn unit() -> TreePrinter<Context> {
        TreePrinter::Unit
    }

    pub fn indent() -> TreePrinter<Context>
    where
        Context: IndentContext + PrintContext,
    {
        TreePrinter::Leaf(Box::new(IndentPrinter))
    }

    pub fn leaf<P: Printer<Context> + 'static>(p: P) -> TreePrinter<Context> {
        TreePrinter::Leaf(Box::new(p))
    }

    pub fn node<P: Printer<Context> + 'static>(self, p: P) -> TreePrinter<Context> {
        match &self {
            TreePrinter::Unit => TreePrinter::leaf(p),
            _ => TreePrinter::Node {
                left: Box::new(self),
                right: Box::new(TreePrinter::leaf(p)),
            },
        }
    }

    pub fn print(&self, ctx: &mut Context) {
        match self {
            TreePrinter::Leaf(p) => p.print(ctx),
            TreePrinter::Node { left, right } => {
                left.print(ctx);
                right.print(ctx);
            }
            TreePrinter::Unit => {}
        }
    }
}

impl<Context, P> Add<Option<P>> for TreePrinter<Context>
where
    TreePrinter<Context>: Add<P, Output = TreePrinter<Context>>,
{
    type Output = TreePrinter<Context>;

    fn add(self, rhs: Option<P>) -> Self::Output {
        match rhs {
            None => self,
            Some(p) => self + p,
        }
    }
}

impl<Context> Add for TreePrinter<Context> {
    type Output = TreePrinter<Context>;

    fn add(self, rhs: TreePrinter<Context>) -> Self::Output {
        match &self {
            TreePrinter::Unit => rhs,
            _ => TreePrinter::Node {
                left: Box::new(self),
                right: Box::new(rhs),
            },
        }
    }
}

impl<Context: PrintContext> Add<&str> for TreePrinter<Context> {
    type Output = TreePrinter<Context>;

    fn add(self, rhs: &str) -> Self::Output {
        self + rhs.to_string()
    }
}

impl<Context: PrintContext> Add<&String> for TreePrinter<Context> {
    type Output = TreePrinter<Context>;

    fn add(self, rhs: &String) -> Self::Output {
        self + rhs.to_string()
    }
}

impl<Context: PrintContext> Add<String> for TreePrinter<Context> {
    type Output = TreePrinter<Context>;

    fn add(self, rhs: String) -> Self::Output {
        self.node(rhs)
    }
}

pub struct IndentedPrinter<Context>(TreePrinter<Context>);

impl<C: IndentContext> Printer<C> for IndentedPrinter<C> {
    fn print(&self, ctx: &mut C) {
        ctx.increment_ident();
        self.0.print(ctx);
        ctx.decrement_ident();
    }
}

pub fn indented<C: IndentContext + 'static>(p: TreePrinter<C>) -> TreePrinter<C> {
    TreePrinter::leaf(IndentedPrinter(p))
}

pub struct NewLine;

impl<C: PrintContext> Printer<C> for NewLine {
    fn print(&self, ctx: &mut C) {
        ctx.print_new_line();
    }
}

impl<C: PrintContext> Add<NewLine> for TreePrinter<C> {
    type Output = TreePrinter<C>;

    fn add(self, rhs: NewLine) -> Self::Output {
        self.node(rhs)
    }
}

#[cfg(test)]
mod tests {
    use crate::printer::{PrintContext, TreePrinter};

    struct StringContext {
        ctx: String,
    }

    impl StringContext {
        fn unit() -> TreePrinter<StringContext> {
            TreePrinter::unit()
        }
    }

    impl PrintContext for StringContext {
        fn print_str(&mut self, s: &str) {
            self.ctx.push_str(s)
        }
    }

    #[test]
    fn str_concat() {
        let p = StringContext::unit() + "a" + "b" + "c";

        let mut ctx = StringContext { ctx: String::new() };

        p.print(&mut ctx);

        assert_eq!(ctx.ctx, "abc")
    }

    #[test]
    fn tree_concat() {
        let p1 = StringContext::unit() + "a" + "b" + "c";
        let p2 = StringContext::unit() + "d" + "e" + "f";
        let p = p1 + p2;

        let mut ctx = StringContext { ctx: String::new() };

        p.print(&mut ctx);

        assert_eq!(ctx.ctx, "abcdef")
    }
}
