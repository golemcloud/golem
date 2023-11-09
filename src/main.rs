use std::path::Path;
use wasm_ast::component::Component;
use wasm_ast::core::{ExprSource, Instr, TryFromExprSource};
use wasmparser::{ComponentExternalKind, Parser, Payload, Validator, WasmFeatures};

fn read_bytes(path: &Path) -> Result<Vec<u8>, std::io::Error> {
    use std::fs::File;
    use std::io::Read;

    let mut file = File::open(path)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(bytes)
}

#[derive(Debug, Clone, PartialEq)]
struct IgnoredExpr {}

impl TryFromExprSource for IgnoredExpr {
    fn try_from<S: ExprSource>(_value: S) -> Result<Self, String>
    where
        Self: Sized,
    {
        Ok(IgnoredExpr {})
    }
}

#[derive(Debug, Clone, PartialEq)]
struct AnalysedExpr {
    contains_table_manipulation: bool,
}

impl TryFromExprSource for AnalysedExpr {
    fn try_from<S: ExprSource>(value: S) -> Result<Self, String>
    where
        Self: Sized,
    {
        let mut found = false;
        for instr in value {
            let instr = instr?;
            match instr {
                Instr::TableSet(_) => {
                    found = true;
                }
                Instr::TableFill(_) => {
                    found = true;
                }
                Instr::TableCopy { .. } => {
                    found = true;
                }
                Instr::TableInit(_, _) => {
                    found = true;
                }
                _ => {}
            }
        }

        Ok(AnalysedExpr {
            contains_table_manipulation: found,
        })
    }
}

fn main() {
    let bytes = read_bytes(Path::new(
        "/Users/vigoo/projects/ziverge/golem/integration-tests/src/it/wasm/shopping-cart.wasm",
    ))
    .unwrap();
    let mut depth = 0;
    let mut found_table_manipulation = false;

    let features = WasmFeatures {
        component_model: true,
        component_model_values: true,
        simd: true,
        ..WasmFeatures::default()
    };

    let mut validator = Validator::new_with_features(features);
    //let mut current_module_sections: Option<Sections<'_, CoreIndexSpace>> = None;

    let parser = Parser::new(0);
    let component: Component<IgnoredExpr> =
        wasm_ast::component::Component::try_from((parser, bytes.as_slice())).unwrap();
    println!("component parsed successfully");
    println!("component metadata {:?}", component.get_metadata());

    let parser = Parser::new(0);
    for payload in parser.parse_all(&bytes) {
        match payload {
            Ok(payload) => {
                //println!("payload: {:?}", payload);
                validator.payload(&payload).unwrap();

                // match &mut current_module_sections {
                //     None => {}
                //     Some(sections) => {
                //         sections.add(&payload);
                //     }
                // }

                match payload {
                    Payload::Version { .. } => {}
                    Payload::TypeSection(_) => {}
                    Payload::ImportSection(_) => {}
                    Payload::FunctionSection(_) => {}
                    Payload::TableSection(_) => {}
                    Payload::MemorySection(_) => {}
                    Payload::TagSection(_) => {}
                    Payload::GlobalSection(_) => {}
                    Payload::ExportSection(_) => {}
                    Payload::StartSection { .. } => {}
                    Payload::ElementSection(_) => {}
                    Payload::DataCountSection { .. } => {}
                    Payload::DataSection(_) => {}
                    Payload::CodeSectionStart { .. } => {}
                    Payload::CodeSectionEntry(body) => {
                        for op in body.get_operators_reader().unwrap() {
                            match op.unwrap() {
                                wasmparser::Operator::TableSet { .. } => {
                                    found_table_manipulation = true;
                                }
                                wasmparser::Operator::TableFill { .. } => {
                                    found_table_manipulation = true;
                                }
                                wasmparser::Operator::TableCopy { .. } => {
                                    found_table_manipulation = true;
                                }
                                wasmparser::Operator::TableInit { .. } => {
                                    found_table_manipulation = true;
                                }
                                _ => {}
                            }
                        }
                    }
                    Payload::ModuleSection { .. } => {
                        depth += 1;
                        //current_module_sections = Some(Sections::new_core());
                    }
                    Payload::InstanceSection(_) => {}
                    Payload::CoreTypeSection(_) => {}
                    Payload::ComponentSection { .. } => {
                        depth += 1;
                    }
                    Payload::ComponentInstanceSection(_) => {}
                    Payload::ComponentAliasSection(_) => {}
                    Payload::ComponentTypeSection(_) => {}
                    Payload::ComponentCanonicalSection(_) => {}
                    Payload::ComponentStartSection { .. } => {}
                    Payload::ComponentImportSection(_) => {}
                    Payload::ComponentExportSection(export_reader) => {
                        if depth == 0 {
                            for exp in export_reader {
                                let exp = exp.unwrap();
                                println!("Export {:?}", exp);

                                match exp.kind {
                                    ComponentExternalKind::Module => {}
                                    ComponentExternalKind::Func => {}
                                    ComponentExternalKind::Value => {}
                                    ComponentExternalKind::Type => {}
                                    ComponentExternalKind::Instance => {
                                        let index = exp.index;
                                        let types = validator.types(0).unwrap();
                                        let component_instance = types.component_instance_at(index);
                                        println!("Component instance {:?}", component_instance);
                                    }
                                    ComponentExternalKind::Component => {}
                                }
                            }
                        }
                    }
                    Payload::CustomSection(_) => {}
                    Payload::UnknownSection { .. } => {}
                    Payload::End(_) => {
                        // match current_module_sections.take() {
                        //     Some(sections) => {
                        //         let module = Module::from_sections(sections);
                        //         println!("Module {:?}", module);
                        //     }
                        //     None => {}
                        // }
                        depth -= 1;
                    }
                }
            }
            Err(err) => {
                println!("error: {:?}", err);
            }
        }
    }

    println!("Found table manipulation: {found_table_manipulation}");
}
