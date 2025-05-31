use std::collections::HashMap;
use std::collections::HashSet;

use crate::grpc;
use crate::grpc::ToCustomKebabCase;
use crate::wit;
use grpc::FileFromUtils;
use serde::{Deserialize, Serialize};

fn fill_builtin_types() -> Vec<wit::Type> {
    let types = vec![
        wit::Type {
            name: "grpc-configuration".to_owned(),
            kind: "record".to_owned(),
            is_record: true,
            fields: vec![
                wit::Field {
                    full_type_name: "".to_owned(),
                    name: "url".to_owned(),
                    type_name: "string".to_owned(),
                },
                wit::Field {
                    full_type_name: "".to_owned(),
                    name: "secret-token".to_owned(),
                    type_name: "option<string>".to_owned(),
                },
            ],
            ..Default::default()
        },
        // grpc status code
        wit::Type {
            name: "grpc-status-code".to_owned(),
            kind: "enum".to_owned(),
            is_enum: true,
            fields: vec![
                wit::Field {
                    full_type_name: "".to_owned(),
                    name: "ok".to_owned(),
                    type_name: "ok".to_owned(),
                },
                wit::Field {
                    full_type_name: "".to_owned(),
                    name: "cancelled".to_owned(),
                    type_name: "cancelled".to_owned(),
                },
                wit::Field {
                    full_type_name: "".to_owned(),
                    name: "unknown".to_owned(),
                    type_name: "unknown".to_owned(),
                },
                wit::Field {
                    full_type_name: "".to_owned(),
                    name: "invalid-argument".to_owned(),
                    type_name: "invalid-argument".to_owned(),
                },
                wit::Field {
                    full_type_name: "".to_owned(),
                    name: "deadline-exceeded".to_owned(),
                    type_name: "deadline-exceeded".to_owned(),
                },
                wit::Field {
                    full_type_name: "".to_owned(),
                    name: "not-found".to_owned(),
                    type_name: "not-found".to_owned(),
                },
                wit::Field {
                    full_type_name: "".to_owned(),
                    name: "already-exists".to_owned(),
                    type_name: "already-exists".to_owned(),
                },
                wit::Field {
                    full_type_name: "".to_owned(),
                    name: "permission-denied".to_owned(),
                    type_name: "permission-denied".to_owned(),
                },
                wit::Field {
                    full_type_name: "".to_owned(),
                    name: "resource-exhausted".to_owned(),
                    type_name: "resource-exhausted".to_owned(),
                },
                wit::Field {
                    full_type_name: "".to_owned(),
                    name: "failed-precondition".to_owned(),
                    type_name: "failed-precondition".to_owned(),
                },
                wit::Field {
                    full_type_name: "".to_owned(),
                    name: "aborted".to_owned(),
                    type_name: "aborted".to_owned(),
                },
                wit::Field {
                    full_type_name: "".to_owned(),
                    name: "out-of-range".to_owned(),
                    type_name: "out-of-range".to_owned(),
                },
                wit::Field {
                    full_type_name: "".to_owned(),
                    name: "unimplemented".to_owned(),
                    type_name: "unimplemented".to_owned(),
                },
                wit::Field {
                    full_type_name: "".to_owned(),
                    name: "internal".to_owned(),
                    type_name: "internal".to_owned(),
                },
                wit::Field {
                    full_type_name: "".to_owned(),
                    name: "unavailable".to_owned(),
                    type_name: "unavailable".to_owned(),
                },
                wit::Field {
                    full_type_name: "".to_owned(),
                    name: "data-loss".to_owned(),
                    type_name: "data-loss".to_owned(),
                },
                wit::Field {
                    full_type_name: "".to_owned(),
                    name: "unauthenticated".to_owned(),
                    type_name: "unauthenticated".to_owned(),
                },
            ],
            ..Default::default()
        },
        // Add the GrpcStatus struct type
        wit::Type {
            name: "grpc-status".to_owned(),
            kind: "record".to_owned(),
            is_record: true,
            fields: vec![
                wit::Field {
                    full_type_name: "".to_owned(),
                    name: "code".to_owned(),
                    type_name: "grpc-status-code".to_owned(),
                },
                wit::Field {
                    full_type_name: "".to_owned(),
                    name: "message".to_owned(),
                    type_name: "string".to_owned(),
                },
                wit::Field {
                    full_type_name: "".to_owned(),
                    name: "details".to_owned(),
                    type_name: "list<u8>".to_owned(),
                },
            ],
            ..Default::default()
        },
    ];

    types
}

pub trait WitUtils {
    fn from_fd(file_descriptor_set: &prost_types::FileDescriptorSet, version: Option<&str>)
        -> Self;
    fn merge_wits(wits: Vec<Wit>) -> Self;
    fn to_string_format(self) -> String;
}

impl WitUtils for Wit {
    fn from_fd(
        file_descriptor_set: &prost_types::FileDescriptorSet,
        version: Option<&str>,
    ) -> Self {
        let mut wits: Vec<wit::Wit> = vec![];
        for file in file_descriptor_set.file.iter() {
            wits.push(Self::from_util(file, version));
        }

        Self::merge_wits(wits)
    }

    fn merge_wits(wits: Vec<Wit>) -> Self {
        let parent_wit = wits.last().expect("Expecting at least one wit");
        let mut interfaces = parent_wit.interfaces.clone();

        let grpc = wit::Interface {
            name: "rpc-grpc".to_owned(),
            types: fill_builtin_types(),
            ..Default::default()
        };

        let mut world = parent_wit.world.clone();

        for wit in wits[0..wits.len() - 1].iter() {
            for interface in wit.interfaces.iter() {
                interfaces.push(interface.clone());
            }

            // Merge interfaces by name, combining their types
            let mut interfaces_map: HashMap<String, Interface> = HashMap::new();
            for interface in interfaces.into_iter() {
                interfaces_map
                    .entry(interface.name.clone())
                    .and_modify(|existing| existing.types.extend(interface.types.clone()))
                    .or_insert(interface);
            }
            interfaces = interfaces_map.into_values().collect();

            // for export in wit.world.exports.iter() {
            //     // push if not exists
            //     if !world.exports.contains(export) {
            //         world.exports.push(export.to_string());
            //     }
            // }
        }

        // merge types
        let mut interfaces_map: HashMap<String, Interface> = HashMap::new();
        for interface in interfaces.into_iter() {
            interfaces_map
                .entry(interface.name.clone())
                .and_modify(|existing| existing.types.extend(interface.types.clone()))
                .or_insert(interface);
        }
        interfaces = interfaces_map.into_values().collect();

        let all_proto_packages: HashSet<String> =
            interfaces.iter().map(|i| i.package.clone()).collect();

        interfaces.iter_mut().for_each(|interface| {
            let mut uses_map: HashMap<String, Uses> = HashMap::new();
            let mut type_names = HashSet::new();

            interface.types.iter().for_each(|type_| {
                type_.fields.iter().for_each(|field| {
                    type_names.insert(field.full_type_name.clone());
                });
            });

            interface.resources.iter().for_each(|resource| {
                resource
                    .constructor
                    .parameters
                    .iter()
                    .for_each(|parameter| {
                        type_names.insert(parameter.full_type_name.clone());
                    });

                resource.functions.iter().for_each(|func| {
                    func.parameters.iter().for_each(|parameter| {
                        type_names.insert(parameter.full_type_name.clone());
                    });
                });

                resource.functions.iter().for_each(|func| {
                    // func.result.iter().for_each(|result| {
                    if let Some(return_type) = &func.result {
                        match return_type {
                            ReturnTypeEnum::Result(result_return_type) => {
                                type_names.insert(result_return_type.full_type_name.clone());
                            }
                            ReturnTypeEnum::Normal(_) => {}
                        };
                    };
                });
            });

            type_names.iter().for_each(|type_name| {
                // trim start with any of the all_proto_packages
                // if possible them push into Uses map
                let result = all_proto_packages.iter().find_map(|p| {
                    if type_name.trim_start_matches(".").starts_with(p)
                        && type_name.split(".").count() >= 2
                    {
                        Some(p)
                    } else {
                        None
                    }
                });
                if let Some(pkg) = result {
                    if pkg != &interface.package || interface.is_service {
                        let pkg_ = pkg.to_custom_kebab_case();
                        let temp1 = type_name
                            .trim_start_matches(&format!(".{}", pkg).to_owned())
                            .trim_start_matches(".")
                            // .replace(".", "-i-")
                            .to_custom_kebab_case()
                            .check_is_wit_keyword();
                        let type_ = format!("{} as {}", temp1, type_name.to_custom_kebab_case());
                        // uses
                        uses_map
                            .entry(pkg_.clone())
                            .and_modify(|uses| uses.type_names.push(type_.clone()))
                            .or_insert_with(|| Uses {
                                interface_name: pkg_,
                                type_names: vec![type_],
                            });
                    };
                };
            });

            interface
                .uses
                .extend(uses_map.clone().into_values().map(|u| u.clone()));
        });

        // fi;; builtin types
        interfaces.push(grpc);

        // // Remove unused types from each interface based on their uses, efficiently
        // let mut interface_used_types: HashMap<String, HashSet<String>> = HashMap::new();

        // // Collect all used types per interface
        // for interface in &interfaces {

        //     for u in &interface.uses {
        //         let used_types = interface_used_types
        //         .entry(u.interface_name.clone())
        //         .or_insert_with(HashSet::new);
        //         for t in &u.type_names {
        //             if let Some(type_name) = t.split(" as ").next() {
        //                 used_types.insert(type_name.to_string());
        //             } else {
        //                 used_types.insert(t.to_owned());
        //             }
        //         }
        //     }
        // }

        //     // also in fields, params, return types
        //     // Collect types used in fields of types
        //     for ty in &interface.types {
        //         for field in &ty.fields {
        //             interface_used_types
        //                 .entry(interface.name.clone())
        //                 .or_insert_with(HashSet::new)
        //                 .insert(extract_inner_type(&field.type_name)); // clean type_name
        //             // may be option<type>, type, list<type> tuple(option<type>, type, list<type>, so om)
        //         }
        //     }

        //     for resource in &interface.resources {
        //         for param in &resource.constructor.parameters {
        //             interface_used_types
        //                 .entry(interface.name.clone())
        //                 .or_insert_with(HashSet::new)
        //                 .insert(extract_inner_type(&param.type_name));
        //         }
        //         for func in &resource.functions {
        //             for param in &func.parameters {
        //                 interface_used_types
        //                     .entry(interface.name.clone())
        //                     .or_insert_with(HashSet::new)
        //                     .insert(extract_inner_type(&param.type_name));
        //             }
        //             if let Some(result) = &func.result {
        //                 interface_used_types
        //                     .entry(interface.name.clone())
        //                     .or_insert_with(HashSet::new)
        //                     .insert(extract_inner_type(&result.ok));
        //                 interface_used_types
        //                     .entry(interface.name.clone())
        //                     .or_insert_with(HashSet::new)
        //                     .insert(extract_inner_type(&result.err));
        //             }
        //         }
        //     }
        // }

        // // Retain only used types for each interface
        // for interface in &mut interfaces {
        //     if let Some(used_types) = interface_used_types.get(&interface.name) {
        //         interface.types.retain(|ty| used_types.contains(&ty.name));
        //     }
        // }

        // need to push interface names to world exports if not present on exports before this step
        for interface in &interfaces {
            if !world.exports.contains(&interface.name) {
                world.exports.push(interface.name.clone());
            }
        }

        Self {
            package_meta: parent_wit.package_meta.clone(),
            interfaces,
            world,
        }
    }

    fn to_string_format(self) -> String {
        let wit_template_str = r#"
package {{package_meta.name_space}}:{{package_meta.name}};

{{#interfaces}}
interface {{name}} {
    {{#uses}}
    use {{interface_name}}.{ {{#type_names}}{{.}}{{^last}}, {{/last}}{{/type_names}}};
    {{/uses}}
    {{#types}}
    {{#is_record}}
    record {{name}} {
        {{#fields}}
        {{name}}: {{{type_name}}},
        {{/fields}}
    }
    {{/is_record}}
    {{#is_enum}}
    enum {{name}} {
        {{#fields}}
        {{name}},
        {{/fields}}
    }
    {{/is_enum}}
    {{#is_variant}}
    variant {{name}} {
        {{#fields}}
        {{name}}({{{type_name}}}),
        {{/fields}}
    }
    {{/is_variant}}
    {{/types}}
    {{#functions}}
    {{name}}: func({{#parameters}}{{name}}: {{{type_name}}}{{^last}}, {{/last}}{{/parameters}}){{#result}} -> result<{{{ok}}}, {{{err}}}>{{/result}};
    {{/functions}}
    {{#resources}}
    resource {{name}} {
        constructor({{#constructor.parameters}}{{name}}: {{{type_name}}}{{^last}}, {{/last}}{{/constructor.parameters}});
        
        {{#functions}}
        {{name}}: func({{#parameters}}{{name}}: {{{type_name}}}{{^last}}, {{/last}}{{/parameters}}) -> result<{{{result.ok}}}, {{{result.err}}}>;

        {{/functions}}
        {{#normal_functions}}
        {{name}}: func({{#parameters}}{{name}}: {{{type_name}}}{{^last}}, {{/last}}{{/parameters}}) -> {{{result}}};

        {{/normal_functions}}
    
    }
{{/resources}}

}

{{/interfaces}}


world {{package_meta.name}}-world {
    {{#world.exports}}
    export {{.}};
    {{/world.exports}}
}

"#;

        let template = mustache::compile_str(wit_template_str).unwrap();

        template.render_to_string(&self).unwrap()
    }
}

// will handel tuple later
fn _extract_inner_type(type_name: &str) -> String {
    let mut t = type_name.trim();
    loop {
        if t.starts_with("option<") && t.ends_with('>') && t.len() > 8 {
            t = &t[7..t.len() - 1];
        } else if t.starts_with("list<") && t.ends_with('>') && t.len() > 6 {
            t = &t[5..t.len() - 1];
        } else {
            break;
        }
        t = t.trim();
    }
    t.to_string()
}

#[derive(Serialize, Deserialize, Default, Clone)]
pub struct Wit {
    pub package_meta: PackageMetadata,
    pub interfaces: Vec<Interface>,
    pub world: World,
}

#[derive(Serialize, Deserialize, Default, Clone)]
pub struct PackageMetadata {
    pub docs: String,
    pub name: String,
    pub name_space: String,
    pub version: String,
}

#[derive(Serialize, Deserialize, Default, Clone)]
pub struct World {
    pub docs: String,
    pub name: String,
    pub exports: Vec<String>,
}

#[derive(Serialize, Deserialize, Default, Clone)]
pub struct Interface {
    pub is_service: bool,
    pub package: String,
    pub docs: String,
    pub name: String,
    pub uses: Vec<Uses>,
    pub types: Vec<Type>,
    pub functions: Vec<Function>,
    pub resources: Vec<Resource>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Uses {
    pub interface_name: String,
    pub type_names: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Function {
    pub docs: String,
    pub name: String,
    pub parameters: Vec<Parameter>,
    pub result: Option<ReturnTypeEnum>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct NormalFunction {
    pub docs: String,
    pub name: String,
    pub parameters: Vec<Parameter>,
    pub result: Option<ReturnTypeEnum>,
}

#[derive(Serialize, Deserialize, Clone)]
pub enum ReturnTypeEnum {
    Result(ResultReturnType),
    Normal(String),
}

#[derive(Serialize, Deserialize, Clone)]
pub struct ResultReturnType {
    pub full_type_name: String,
    pub ok: String,
    pub err: String,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Parameter {
    pub full_type_name: String,
    pub name: String,
    pub type_name: String,
}

#[derive(Serialize, Deserialize, Default, Clone)]
pub struct Resource {
    pub docs: String,
    pub name: String,
    pub constructor: Constructor,
    pub functions: Vec<Function>,
    pub normal_functions: Vec<NormalFunction>
}

#[derive(Serialize, Deserialize, Default, Clone)]
pub struct Constructor {
    pub name: String,
    pub parameters: Vec<Parameter>,
}

#[derive(Serialize, Deserialize, Default, Clone)]
pub struct Type {
    pub docs: String,
    pub name: String,
    pub kind: String,
    pub is_record: bool,
    pub is_enum: bool,
    pub is_variant: bool,
    pub fields: Vec<Field>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Field {
    pub full_type_name: String,
    pub name: String,
    pub type_name: String,
}
