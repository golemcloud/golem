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

use crate::bridge_gen::typescript::type_name::TypeScriptTypeName;
use anyhow::anyhow;
use camino::Utf8Path;
use std::collections::BTreeSet;

pub trait FunctionWriter {
    fn param(&mut self, name: &str, typ: &str);
    fn result(&mut self, typ: &str);
    fn write(&mut self, content: impl AsRef<str>);
    fn write_line(&mut self, line: impl AsRef<str>);
    fn indent(&mut self);
    fn unindent(&mut self);
}

struct TsModuleState {
    imports: BTreeSet<String>,
    content: String,
}

pub struct TsWriter {
    content: String,
    current_indent: usize,
    module_stack: Vec<TsModuleState>,
}

impl TsWriter {
    pub fn new() -> Self {
        TsWriter {
            content: String::new(),
            current_indent: 0,
            module_stack: Vec::new(),
        }
    }

    pub fn finish(self, target: &Utf8Path) -> anyhow::Result<()> {
        std::fs::write(target, self.content)
            .map_err(|e| anyhow!("Failed to write TypeScript file: {e}"))
    }

    pub fn write_doc(&mut self, doc: &str) {
        let doc = doc.trim_end();
        if doc.is_empty() {
            return;
        }
        self.indented_write_line("/**");
        for line in doc.lines() {
            if line.trim().is_empty() {
                self.indented_write_line(" *");
            } else {
                self.indented_write_line(format!(" * {}", line));
            }
        }
        self.indented_write_line(" */");
    }

    pub fn begin_export_namespace(&mut self, name: &str) {
        self.indented_write_line(format!("export namespace {name} {{"));
        self.current_indent += 1;
        self.module_stack.push(TsModuleState {
            imports: BTreeSet::new(),
            content: String::new(),
        })
    }

    pub fn end_export_namespace(&mut self) {
        let module = self.module_stack.pop().expect("No module state to end");
        for import in module.imports {
            self.indented_write_line(import);
        }
        self.write(module.content);

        self.current_indent -= 1;
        self.indented_write_line("}");
    }

    pub fn begin_function(&mut self, name: &str) -> TsFunctionWriter<'_> {
        self.indented_write(format!("function {name}("));
        TsFunctionWriter {
            writer: self,
            param_count: 0,
            return_type: Some("void".to_string()),
            returns_promise: false,
            body: vec![],
            indent_level: 1,
        }
    }

    pub fn begin_export_function(&mut self, name: &str) -> TsFunctionWriter<'_> {
        self.indented_write(format!("export function {name}("));
        TsFunctionWriter {
            writer: self,
            param_count: 0,
            return_type: Some("void".to_string()),
            returns_promise: false,
            body: vec![],
            indent_level: 1,
        }
    }

    pub fn begin_export_async_function(&mut self, name: &str) -> TsFunctionWriter<'_> {
        self.indented_write(format!("export async function {name}("));
        TsFunctionWriter {
            writer: self,
            param_count: 0,
            return_type: Some("void".to_string()),
            returns_promise: true,
            body: vec![],
            indent_level: 1,
        }
    }

    pub fn begin_method(&mut self, name: &str) -> TsFunctionWriter<'_> {
        self.indented_write(format!("{name}("));
        TsFunctionWriter {
            writer: self,
            param_count: 0,
            return_type: Some("void".to_string()),
            returns_promise: false,
            body: vec![],
            indent_level: 1,
        }
    }

    pub fn begin_private_method(&mut self, name: &str) -> TsFunctionWriter<'_> {
        self.indented_write(format!("private {name}("));
        TsFunctionWriter {
            writer: self,
            param_count: 0,
            return_type: Some("void".to_string()),
            returns_promise: false,
            body: vec![],
            indent_level: 1,
        }
    }

    pub fn begin_async_method(&mut self, name: &str) -> TsFunctionWriter<'_> {
        self.indented_write(format!("async {name}("));
        TsFunctionWriter {
            writer: self,
            param_count: 0,
            return_type: Some("void".to_string()),
            returns_promise: true,
            body: vec![],
            indent_level: 1,
        }
    }

    pub fn begin_static_method(&mut self, name: &str) -> TsFunctionWriter<'_> {
        self.indented_write(format!("static {name}("));
        TsFunctionWriter {
            writer: self,
            param_count: 0,
            return_type: Some("void".to_string()),
            returns_promise: false,
            body: vec![],
            indent_level: 1,
        }
    }

    pub fn begin_static_async_method(&mut self, name: &str) -> TsFunctionWriter<'_> {
        self.indented_write(format!("static async {name}("));
        TsFunctionWriter {
            writer: self,
            param_count: 0,
            return_type: Some("void".to_string()),
            returns_promise: true,
            body: vec![],
            indent_level: 1,
        }
    }

    pub fn begin_constructor(&mut self) -> TsFunctionWriter<'_> {
        self.indented_write("constructor(");
        TsFunctionWriter {
            writer: self,
            param_count: 0,
            return_type: None,
            returns_promise: false,
            body: vec![],
            indent_level: 1,
        }
    }

    pub fn begin_private_constructor(&mut self) -> TsFunctionWriter<'_> {
        self.indented_write("private constructor(");
        TsFunctionWriter {
            writer: self,
            param_count: 0,
            return_type: None,
            returns_promise: false,
            body: vec![],
            indent_level: 1,
        }
    }

    pub fn begin_export_class(&mut self, name: &str) {
        self.indented_write(format!("export class {name} {{\n"));
        self.current_indent += 1;
    }

    pub fn end_export_class(&mut self) {
        self.current_indent -= 1;
        self.indented_write_line("}");
    }

    pub fn export_type(&mut self, name: &TypeScriptTypeName, definition: &str) {
        self.indented_write_line(format!("export type {name} = {definition};"));
    }

    pub fn declare_global(&mut self, name: &str, typ: &str, default_value: Option<&str>) {
        if let Some(default_value) = default_value {
            self.indented_write_line(format!("var {name}: {typ} = {default_value};"));
        } else {
            self.indented_write_line(format!("var {name}: {typ};"));
        }
    }

    pub fn declare_field(&mut self, name: &str, typ: &str, default_value: Option<&str>) {
        if let Some(default_value) = default_value {
            self.indented_write_line(format!("readonly {name}: {typ} = {default_value};"));
        } else {
            self.indented_write_line(format!("readonly {name}: {typ};"));
        }
    }

    pub fn import_module(&mut self, name: &str, from: &str) {
        let import_line = format!("import * as {name} from '{from}';");
        if let Some(module) = self.module_stack.last_mut() {
            module.imports.insert(import_line);
        } else {
            self.indented_write_line(import_line);
        }
    }

    pub fn import_item(&mut self, item: &str, alias: &str, from: &str) {
        let import_line = format!("import {{ {item} as {alias} }} from '{from}';");
        if let Some(module) = self.module_stack.last_mut() {
            module.imports.insert(import_line);
        } else {
            self.indented_write_line(import_line);
        }
    }

    pub fn write_line(&mut self, line: impl AsRef<str>) {
        self.indented_write_line(line);
    }

    pub fn indent(&mut self) {
        self.current_indent += 1;
    }

    pub fn unindent(&mut self) {
        if self.current_indent > 0 {
            self.current_indent -= 1;
        }
    }

    fn indented_write_line(&mut self, line: impl AsRef<str>) {
        for line in line.as_ref().lines() {
            self.indented_write(line);
            self.write("\n");
        }
    }

    fn indented_write(&mut self, line: impl AsRef<str>) {
        let indent = "  ".repeat(self.current_indent);
        self.write(format!("{}{}", indent, line.as_ref()));
    }

    fn write(&mut self, content: impl AsRef<str>) {
        if let Some(module) = self.module_stack.last_mut() {
            module.content.push_str(content.as_ref());
        } else {
            self.content.push_str(content.as_ref());
        }
    }
}

pub struct TsFunctionWriter<'a> {
    writer: &'a mut TsWriter,
    param_count: usize,
    return_type: Option<String>,
    returns_promise: bool,
    body: Vec<String>,
    indent_level: usize,
}

impl<'a> TsFunctionWriter<'a> {
    pub fn param(&mut self, name: &str, typ: &str) {
        if self.param_count > 0 {
            self.writer.write(", ");
        }
        self.writer.write(format!("{name}: {typ}"));
        self.param_count += 1;
    }

    pub fn result(&mut self, typ: &str) {
        self.return_type = Some(typ.to_string());
    }

    pub fn write(&mut self, content: impl AsRef<str>) {
        let lines = content.as_ref().lines().collect::<Vec<_>>();
        if self.body.is_empty() {
            for line in lines {
                self.body.push(indent(line, self.indent_level * 2));
            }
        } else {
            let last_line = self.body.last_mut().unwrap();
            let (_, new_rest) = lines.split_first().unwrap();
            last_line.push_str(content.as_ref());
            for line in new_rest {
                self.body.push(indent(line, self.indent_level * 2));
            }
        }
    }

    pub fn write_line(&mut self, line: impl AsRef<str>) {
        self.body.push(indent(line.as_ref(), self.indent_level * 2));
    }

    pub fn indent(&mut self) {
        self.indent_level += 1;
    }

    pub fn unindent(&mut self) {
        if self.indent_level > 0 {
            self.indent_level -= 1;
        }
    }
}

impl<'a> FunctionWriter for TsFunctionWriter<'a> {
    fn param(&mut self, name: &str, typ: &str) {
        TsFunctionWriter::param(self, name, typ);
    }

    fn result(&mut self, typ: &str) {
        TsFunctionWriter::result(self, typ);
    }

    fn write(&mut self, content: impl AsRef<str>) {
        TsFunctionWriter::write(self, content);
    }

    fn write_line(&mut self, line: impl AsRef<str>) {
        TsFunctionWriter::write_line(self, line);
    }

    fn indent(&mut self) {
        TsFunctionWriter::indent(self);
    }

    fn unindent(&mut self) {
        TsFunctionWriter::unindent(self);
    }
}

impl<'a> Drop for TsFunctionWriter<'a> {
    fn drop(&mut self) {
        self.writer.write(")");
        if let Some(return_type) = &self.return_type {
            self.writer.write(": ");

            if self.returns_promise {
                self.writer.write("Promise<");
            }
            self.writer.write(return_type);
            if self.returns_promise {
                self.writer.write(">");
            }
        }

        self.writer.write(" {\n");
        for line in &self.body {
            self.writer.indented_write_line(line);
        }
        self.writer.indented_write_line("}");
    }
}

pub fn indent(s: &str, spaces: usize) -> String {
    let indent = " ".repeat(spaces);
    s.lines()
        .map(|line| format!("{indent}{line}\n"))
        .collect::<String>()
}

pub struct TsAnonymousFunctionWriter {
    params: Vec<String>,
    return_type: Option<String>,
    returns_promise: bool,
    body: Vec<String>,
    indent_level: usize,
}

impl TsAnonymousFunctionWriter {
    pub fn new() -> Self {
        TsAnonymousFunctionWriter {
            params: Vec::new(),
            return_type: None,
            returns_promise: false,
            body: vec![],
            indent_level: 1,
        }
    }

    pub fn async_fn() -> Self {
        TsAnonymousFunctionWriter {
            params: Vec::new(),
            return_type: None,
            returns_promise: true,
            body: vec![],
            indent_level: 1,
        }
    }

    pub fn param(&mut self, name: &str, typ: &str) {
        self.params.push(format!("{name}: {typ}"));
    }

    pub fn result(&mut self, typ: &str) {
        self.return_type = Some(typ.to_string());
    }

    pub fn write(&mut self, content: impl AsRef<str>) {
        let lines = content.as_ref().lines().collect::<Vec<_>>();
        if self.body.is_empty() {
            for line in lines {
                self.body.push(indent(line, self.indent_level * 2));
            }
        } else {
            let last_line = self.body.last_mut().unwrap();
            let (new_head, new_rest) = lines.split_first().unwrap();
            last_line.push_str(new_head);
            for line in new_rest {
                self.body.push(indent(line, self.indent_level * 2));
            }
        }
    }

    pub fn write_line(&mut self, line: impl AsRef<str>) {
        self.body.push(indent(line.as_ref(), self.indent_level * 2));
    }

    pub fn indent(&mut self) {
        self.indent_level += 1;
    }

    pub fn unindent(&mut self) {
        if self.indent_level > 0 {
            self.indent_level -= 1;
        }
    }

    pub fn build(self) -> String {
        let mut result = String::new();

        // Write function signature
        result.push('(');
        result.push_str(&self.params.join(", "));
        result.push(')');

        // Write return type
        if let Some(return_type) = &self.return_type {
            result.push_str(": ");
            if self.returns_promise {
                result.push_str("Promise<");
            }
            result.push_str(return_type);
            if self.returns_promise {
                result.push('>');
            }
        }

        result.push_str(" => {\n");

        // Write body
        for line in &self.body {
            result.push_str(line);
        }

        result.push_str("}\n");

        result
    }
}

impl FunctionWriter for TsAnonymousFunctionWriter {
    fn param(&mut self, name: &str, typ: &str) {
        TsAnonymousFunctionWriter::param(self, name, typ);
    }

    fn result(&mut self, typ: &str) {
        TsAnonymousFunctionWriter::result(self, typ);
    }

    fn write(&mut self, content: impl AsRef<str>) {
        TsAnonymousFunctionWriter::write(self, content);
    }

    fn write_line(&mut self, line: impl AsRef<str>) {
        TsAnonymousFunctionWriter::write_line(self, line);
    }

    fn indent(&mut self) {
        TsAnonymousFunctionWriter::indent(self);
    }

    fn unindent(&mut self) {
        TsAnonymousFunctionWriter::unindent(self);
    }
}
