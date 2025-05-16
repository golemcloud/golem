use crate::grpc;
use crate::wit;
use grpc::FileFromUtils;
use serde::{Deserialize, Serialize};

fn fill_root_types() -> Vec<wit::Type> {
    let types = vec![
        wit::Type {
            name: "grpc-configuration".to_owned(),
            kind: "record".to_owned(),
            is_record: true,
            fields: vec![
                wit::Field {
                    name: "url".to_owned(),
                    type_name: "string".to_owned(),
                },
                wit::Field {
                    name: "secret-token".to_owned(),
                    type_name: "string".to_owned(),
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
                    name: "ok".to_owned(),
                    type_name: "ok".to_owned(),
                },
                wit::Field {
                    name: "cancelled".to_owned(),
                    type_name: "cancelled".to_owned(),
                },
                wit::Field {
                    name: "unknown".to_owned(),
                    type_name: "unknown".to_owned(),
                },
                wit::Field {
                    name: "invalid-argument".to_owned(),
                    type_name: "invalid-argument".to_owned(),
                },
                wit::Field {
                    name: "deadline-exceeded".to_owned(),
                    type_name: "deadline-exceeded".to_owned(),
                },
                wit::Field {
                    name: "not-found".to_owned(),
                    type_name: "not-found".to_owned(),
                },
                wit::Field {
                    name: "already-exists".to_owned(),
                    type_name: "already-exists".to_owned(),
                },
                wit::Field {
                    name: "permission-denied".to_owned(),
                    type_name: "permission-denied".to_owned(),
                },
                wit::Field {
                    name: "resource-exhausted".to_owned(),
                    type_name: "resource-exhausted".to_owned(),
                },
                wit::Field {
                    name: "failed-precondition".to_owned(),
                    type_name: "failed-precondition".to_owned(),
                },
                wit::Field {
                    name: "aborted".to_owned(),
                    type_name: "aborted".to_owned(),
                },
                wit::Field {
                    name: "out-of-range".to_owned(),
                    type_name: "out-of-range".to_owned(),
                },
                wit::Field {
                    name: "unimplemented".to_owned(),
                    type_name: "unimplemented".to_owned(),
                },
                wit::Field {
                    name: "internal".to_owned(),
                    type_name: "internal".to_owned(),
                },
                wit::Field {
                    name: "unavailable".to_owned(),
                    type_name: "unavailable".to_owned(),
                },
                wit::Field {
                    name: "data-loss".to_owned(),
                    type_name: "data-loss".to_owned(),
                },
                wit::Field {
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
                    name: "code".to_owned(),
                    type_name: "grpc-status-code".to_owned(),
                },
                wit::Field {
                    name: "message".to_owned(),
                    type_name: "string".to_owned(),
                },
                wit::Field {
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
        let world = parent_wit.world.clone();

        for interface in interfaces.iter_mut() {
            if interface
                .name
                .eq_ignore_ascii_case(world.name.trim_end_matches("-world"))
            {
                interface.types.extend(fill_root_types());
            }
        }
        let mut world = parent_wit.world.clone();

        for wit in wits[0..wits.len() - 1].iter() {
            let package_name = &wit.world.name.trim_end_matches("-world");

            interfaces.iter_mut().for_each(|interface| {
                if let Some(uses) = interface
                    .uses
                    .iter_mut()
                    .find(|uses| uses.interface_name.eq_ignore_ascii_case(package_name))
                {
                    uses.type_names.iter_mut().for_each(|type_name| {
                        *type_name =
                            type_name.to_string() + " as " + package_name + "-" + type_name;
                    });
                }
            });

            for interface in wit.interfaces.iter() {
                interfaces.push(interface.clone());
            }

            for export in wit.world.exports.iter() {
                world.exports.push(export.to_string());
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
    pub result: Option<ReturnType>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct ReturnType {
    pub ok: String,
    pub err: String,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Parameter {
    pub name: String,
    pub type_name: String,
}

#[derive(Serialize, Deserialize, Default, Clone)]
pub struct Resource {
    pub docs: String,
    pub name: String,
    pub constructor: Constructor,
    pub functions: Vec<Function>,
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
    pub name: String,
    pub type_name: String,
}
