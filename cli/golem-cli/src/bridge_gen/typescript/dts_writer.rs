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

use anyhow::anyhow;
use camino::Utf8Path;
use std::collections::BTreeSet;

struct DtsModuleState {
    imports: BTreeSet<String>,
    content: String,
}

pub struct DtsWriter {
    content: String,
    current_indent: usize,
    module_stack: Vec<DtsModuleState>,
}

impl DtsWriter {
    pub fn new() -> Self {
        DtsWriter {
            content: String::new(),
            current_indent: 0,
            module_stack: Vec::new(),
        }
    }

    pub fn finish(self, target: &Utf8Path) -> anyhow::Result<()> {
        std::fs::write(target, self.content)
            .map_err(|e| anyhow!("Failed to write TypeScript definitions: {e}"))
    }

    pub fn begin_declare_module(&mut self, name: &str) {
        self.indented_write_line(format!("declare module '{name}' {{"));
        self.current_indent += 1;
        self.module_stack.push(DtsModuleState {
            imports: BTreeSet::new(),
            content: String::new(),
        })
    }

    pub fn begin_export_namespace(&mut self, name: &str) {
        self.indented_write_line(format!("export namespace {name} {{"));
        self.current_indent += 1;
    }

    pub fn end_declare_module(&mut self) {
        let module = self.module_stack.pop().expect("No module state to end");
        // Write all imports collected in this module
        for import in module.imports {
            self.indented_write_line(import);
        }
        // Write all lines collected in this module
        self.write(module.content);

        self.current_indent -= 1;
        self.indented_write_line("}");
    }

    pub fn end_export_module(&mut self) {
        self.current_indent -= 1;
        self.indented_write_line("}");
    }

    pub fn begin_export_function<'a>(&'a mut self, name: &str) -> DtsFunctionWriter<'a> {
        self.indented_write(format!("export function {name}("));
        DtsFunctionWriter {
            writer: self,
            param_count: 0,
            return_type: Some("void".to_string()),
            returns_promise: false,
        }
    }

    pub fn begin_export_async_function<'a>(&'a mut self, name: &str) -> DtsFunctionWriter<'a> {
        self.indented_write(format!("export function {name}("));
        DtsFunctionWriter {
            writer: self,
            param_count: 0,
            return_type: Some("void".to_string()),
            returns_promise: true,
        }
    }

    pub fn begin_method<'a>(&'a mut self, name: &str) -> DtsFunctionWriter<'a> {
        self.indented_write(format!("{name}("));
        DtsFunctionWriter {
            writer: self,
            param_count: 0,
            return_type: Some("void".to_string()),
            returns_promise: false,
        }
    }

    pub fn begin_async_method<'a>(&'a mut self, name: &str) -> DtsFunctionWriter<'a> {
        self.indented_write(format!("{name}("));
        DtsFunctionWriter {
            writer: self,
            param_count: 0,
            return_type: Some("void".to_string()),
            returns_promise: true,
        }
    }

    pub fn begin_static_method<'a>(&'a mut self, name: &str) -> DtsFunctionWriter<'a> {
        self.indented_write(format!("static {name}("));
        DtsFunctionWriter {
            writer: self,
            param_count: 0,
            return_type: Some("void".to_string()),
            returns_promise: false,
        }
    }

    pub fn begin_static_async_method<'a>(&'a mut self, name: &str) -> DtsFunctionWriter<'a> {
        self.indented_write(format!("static {name}("));
        DtsFunctionWriter {
            writer: self,
            param_count: 0,
            return_type: Some("void".to_string()),
            returns_promise: true,
        }
    }

    pub fn begin_constructor<'a>(&'a mut self) -> DtsFunctionWriter<'a> {
        self.indented_write("constructor(");
        DtsFunctionWriter {
            writer: self,
            param_count: 0,
            return_type: None,
            returns_promise: false,
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

    pub fn export_type(&mut self, name: &str, definition: &str) {
        self.indented_write_line(format!("export type {name} = {definition};"));
    }

    pub fn import_module(&mut self, name: &str, from: &str) {
        let import_line = format!("import * as {name} from '{from}';");
        if let Some(module) = self.module_stack.last_mut() {
            module.imports.insert(import_line);
        } else {
            self.indented_write_line(import_line);
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

pub struct DtsFunctionWriter<'a> {
    writer: &'a mut DtsWriter,
    param_count: usize,
    return_type: Option<String>,
    returns_promise: bool,
}

impl<'a> DtsFunctionWriter<'a> {
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
}

impl<'a> Drop for DtsFunctionWriter<'a> {
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
        self.writer.write(";\n");
    }
}

pub fn indent(s: &str, spaces: usize) -> String {
    let indent = " ".repeat(spaces);
    s.lines()
        .map(|line| format!("{indent}{line}\n"))
        .collect::<String>()
}
