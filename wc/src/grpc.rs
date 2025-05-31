use crate::wit::{self};
use heck::ToKebabCase;
use prost_types::{FileDescriptorProto, MethodDescriptorProto, OneofDescriptorProto};
use std::{
    collections::{HashMap, HashSet},
    vec,
};

fn get_rpc_type(method: &MethodDescriptorProto) -> String {
    match method.server_streaming() && method.client_streaming() {
        true => "bidirectional-streaming".to_string(),
        false => match method.server_streaming() && !method.client_streaming() {
            true => "server-streaming".to_string(),
            false => match method.client_streaming() && !method.server_streaming() {
                true => "client-streaming".to_string(),
                false => "unary".to_string(),
            },
        },
    }
}

pub trait FileFromUtils {
    fn from_util(file_descriptor_proto: &FileDescriptorProto, version: Option<&str>) -> Self;
}

pub trait OneofFromUtils {
    fn from_util(file_descriptor_proto: &OneofDescriptorProto, name: &str) -> Self;
}

trait MessageFromUtils {
    fn from_util(
        message: &prost_types::DescriptorProto,
        package_name: &str,
        name: &str,
        lazy_resources: &mut Vec<wit::Resource>,
    ) -> Self;
}

trait NestedMessageFromUtils {
    fn from_util_nested(
        types: &mut Vec<wit::Type>,
        message: &prost_types::DescriptorProto,
        package_name: &str,
        nested_name: &str,
        lazy_resources: &mut Vec<wit::Resource>,
    );
}
trait FieldFromUtils {
    fn from_util(
        field: &prost_types::FieldDescriptorProto,
        package_name: &str,
        key: &str,
        lazy_resources: &mut Vec<wit::Resource>,
    ) -> Self;
}

trait EnumFromUtils {
    fn from_util(field: &prost_types::EnumDescriptorProto, package_name: &str, name: &str) -> Self;
}

trait InterfaceFromUtils {
    fn from_util(service: &prost_types::ServiceDescriptorProto, package_name: &str) -> Self;
}

trait ResourceFromUtils {
    fn from_util(
        service: &prost_types::ServiceDescriptorProto,
        unary_methods: Vec<&prost_types::MethodDescriptorProto>,
        package_name: &str,
    ) -> Self;
    fn from_util_non_unary(
        service: &prost_types::ServiceDescriptorProto,
        method: &prost_types::MethodDescriptorProto,
        package_name: &str,
    ) -> Self;
}

impl FileFromUtils for wit::Wit {
    fn from_util(file_descriptor_proto: &FileDescriptorProto, version: Option<&str>) -> Self {
        let package_name = file_descriptor_proto.package();
        let mut processed_package_name = sanitize_type_name(package_name);

        let wit_package_version = match version {
            Some(v) => v.to_string(),
            None => {
                let package_name_parts: Vec<&str> = package_name.split(".").collect();

                let package_name_last_part = *package_name_parts.last().unwrap();

                let version_regex = regex::Regex::new(r"^v\d+(_\d+){0,2}$").unwrap();

                if version_regex.is_match(package_name_last_part) {
                    processed_package_name = processed_package_name
                        .trim_end_matches(&(".".to_owned() + package_name_last_part))
                        .to_string();
                    sanitize_package_version(package_name_last_part)
                } else {
                    "0.1.0".to_string()
                }
            }
        };
        let wit_package_name = processed_package_name
            .replace(".", "-")
            .to_custom_kebab_case();

        let mut types = vec![];
        let mut lazy_resources: Vec<wit::Resource> = vec![];

        file_descriptor_proto
            .message_type
            .iter()
            .for_each(|message| {
                wit::Type::from_util_nested(
                    &mut types,
                    message,
                    package_name,
                    message.name(),
                    &mut lazy_resources,
                );
            });

        file_descriptor_proto.enum_type.iter().for_each(|enum_| {
            types.push(<wit::Type as EnumFromUtils>::from_util(
                enum_,
                package_name,
                enum_.name(),
            ));
        });

        let types_interface = wit::Interface {
            // add package name to types interface name
            package: package_name.to_owned(),
            name: package_name.to_custom_kebab_case().check_is_wit_keyword(),
            docs: package_name.to_owned(),
            types,
            resources: lazy_resources,
            ..Default::default()
        };

        // iter all types interfaces if any type uses foriegn type, place them in uses

        let other_interfaces: Vec<wit::Interface> = file_descriptor_proto
            .service
            .iter()
            .map(|service| wit::Interface::from_util(service, package_name))
            .collect();

        let mut interfaces = vec![types_interface];
        interfaces.extend(other_interfaces);

        let exports = interfaces
            .iter()
            .map(|interface| interface.name.clone())
            .collect();

        Self {
            package_meta: wit::PackageMetadata {
                name_space: "rpc-grpc".to_string(),
                name: wit_package_name.to_string(),
                version: wit_package_version,
                docs: package_name.to_owned(),
            },

            interfaces,

            world: wit::World {
                name: package_name.to_custom_kebab_case() + "-world",
                exports,
                ..Default::default()
            },
        }
    }
}

impl InterfaceFromUtils for wit::Interface {
    fn from_util(service: &prost_types::ServiceDescriptorProto, package_name: &str) -> Self {
        let mut uses: HashMap<String, HashSet<String>> = HashMap::new();

        // });
        uses.entry("rpc-grpc".to_owned())
            .or_default()
            .insert("grpc-configuration as rpc-grpc-configuration".to_owned());
        uses.entry("rpc-grpc".to_owned())
            .or_default()
            .insert("grpc-status as rpc-grpc-status".to_owned());

        let unary_methods: Vec<&prost_types::MethodDescriptorProto> = service
            .method
            .iter()
            .filter(|method| get_rpc_type(method) == "unary")
            .collect();

        let remaining_methods: Vec<&prost_types::MethodDescriptorProto> = service
            .method
            .iter()
            .filter(|method| get_rpc_type(method) != "unary")
            .collect();

        let mut resources = if !unary_methods.is_empty() {
            vec![wit::Resource::from_util(
                service,
                unary_methods,
                package_name,
            )]
        } else {
            vec![]
        };

        remaining_methods.iter().for_each(|method| {
            resources.push(wit::Resource::from_util_non_unary(
                service,
                method,
                package_name,
            ))
        });

        Self {
            package: package_name.to_owned(),
            name: service.name().to_custom_kebab_case().check_is_wit_keyword(),
            resources,
            functions: vec![],
            uses: uses
                .iter()
                .map(|uses_| wit::Uses {
                    interface_name: uses_.0.to_custom_kebab_case(),
                    type_names: uses_.1.iter().cloned().collect(),
                })
                .collect(),
            types: vec![],
            docs: service.name().to_owned(),
            is_service: true,
        }
    }
}

impl EnumFromUtils for wit::Type {
    fn from_util(enum_: &prost_types::EnumDescriptorProto, _package: &str, name: &str) -> Self {
        Self {
            name: format!("{}", name)
                .to_custom_kebab_case()
                .check_is_wit_keyword(),
            docs: enum_.name().replace("*", "."),
            kind: "enum".to_owned(),
            is_enum: true,
            fields: enum_
                .value
                .iter()
                .map(|enum_value| {
                    let enum_value_ = enum_value
                        .name()
                        .replace("_", "-")
                        .split("-")
                        .map(|item| {
                            if item.chars().next().map_or(false, |c| c.is_ascii_digit()) {
                                // if its digit at index 0 then prefix item with -e-
                                format!("e{}", item)
                            } else {
                                item.to_owned()
                            }
                        })
                        .collect::<Vec<String>>()
                        .join("-");
                    wit::Field {
                        full_type_name: "".to_string(),
                        name: enum_value_.clone().check_is_wit_keyword(),
                        type_name: enum_value_.check_is_wit_keyword(),
                    }
                })
                .collect(),
            ..Default::default()
        }
    }
}

impl OneofFromUtils for wit::Type {
    fn from_util(_onof: &prost_types::OneofDescriptorProto, name: &str) -> Self {
        Self {
            name: name.to_custom_kebab_case().check_is_wit_keyword(),
            kind: "variant".to_owned(),
            is_variant: true,
            ..Default::default()
        }
    }
}

// from Field
impl FieldFromUtils for wit::Field {
    fn from_util(
        field: &prost_types::FieldDescriptorProto,
        package_name: &str,
        key: &str,
        lazy_resources: &mut Vec<wit::Resource>,
    ) -> Self {
        let trim = ".".to_string() + package_name;

        let mut type_ = match field.r#type() {
            prost_types::field_descriptor_proto::Type::Double => "f64".to_owned(),
            prost_types::field_descriptor_proto::Type::Float => "f32".to_owned(),
            prost_types::field_descriptor_proto::Type::Int64 => "s64".to_owned(),
            prost_types::field_descriptor_proto::Type::Uint64 => "u64".to_owned(),
            prost_types::field_descriptor_proto::Type::Int32 => "s32".to_owned(),
            prost_types::field_descriptor_proto::Type::Fixed64 => "f64".to_owned(),
            prost_types::field_descriptor_proto::Type::Fixed32 => "f32".to_owned(),
            prost_types::field_descriptor_proto::Type::Bool => "bool".to_owned(),
            prost_types::field_descriptor_proto::Type::String => "string".to_owned(),
            prost_types::field_descriptor_proto::Type::Group => todo!(),
            prost_types::field_descriptor_proto::Type::Message => {
                field
                    .type_name()
                    .trim_start_matches(&trim)
                    .trim_start_matches(".")
                    // .replace(".", "-i-")
                    .to_custom_kebab_case()
                    .check_is_wit_keyword()
            }
            prost_types::field_descriptor_proto::Type::Bytes => "list<u8>".to_owned(),
            prost_types::field_descriptor_proto::Type::Uint32 => "u32".to_owned(),
            prost_types::field_descriptor_proto::Type::Enum => field
                .type_name()
                .trim_start_matches(&trim)
                .trim_start_matches(".")
                // .replace(".", "-i-")
                .to_custom_kebab_case()
                .check_is_wit_keyword(),
            prost_types::field_descriptor_proto::Type::Sfixed32 => "f32".to_owned(),
            prost_types::field_descriptor_proto::Type::Sfixed64 => "f64".to_owned(),
            prost_types::field_descriptor_proto::Type::Sint32 => "s32".to_owned(),
            prost_types::field_descriptor_proto::Type::Sint64 => "s64".to_owned(),
        };

        if field.type_name() == key {
            
            // recurisve type handling with lazy resource

            lazy_resources.push(wit::Resource {
                name: format!("lazy-{}", type_.clone()),
                constructor: wit::Constructor {
                    name: "constructor".to_owned(),
                    parameters: vec![wit::Parameter {
                        full_type_name: "".to_owned(),
                        name: type_.clone(),
                        type_name: type_.clone(),
                    }],
                },
                normal_functions: vec![wit::NormalFunction {
                    name: "get".to_owned(),
                    parameters: vec![],
                    result: Some(wit::ReturnTypeEnum::Normal(type_.to_owned())),
                    docs: "".to_owned(),
                }],
                ..Default::default()
            });
            type_ = format!("lazy-{}", type_);
        };

        Self {
            full_type_name: field.type_name().to_owned(),
            name: field.name().to_custom_kebab_case().check_is_wit_keyword(),
            type_name: match field.label() {
                prost_types::field_descriptor_proto::Label::Optional => {
                    format!("option<{}>", &type_)
                }
                prost_types::field_descriptor_proto::Label::Required => type_,
                prost_types::field_descriptor_proto::Label::Repeated => format!("list<{}>", &type_),
            },
        }
    }
}

impl NestedMessageFromUtils for wit::Type {
    fn from_util_nested(
        types: &mut Vec<wit::Type>,
        message: &prost_types::DescriptorProto,
        package_name: &str,
        name: &str,
        lazy_resources: &mut Vec<wit::Resource>,
    ) {
        if !message.nested_type.is_empty() {
            message.nested_type.iter().for_each(|nested| {
                let nested = nested.clone();
                // nested.name = ;
                wit::Type::from_util_nested(
                    types,
                    &nested.clone(),
                    package_name,
                    &format!("{}.{}", name, nested.name()),
                    lazy_resources,
                );
            });
        };

        if !message.enum_type.is_empty() {
            message.enum_type.iter().for_each(|nested| {
                let nest = nested.clone();

                types.push(<wit::Type as EnumFromUtils>::from_util(
                    &nest.clone(),
                    package_name,
                    &format!("{}.{}", name, nested.name()),
                ));
            });
        };
        if !message.oneof_decl.is_empty() {
            let mut one_of_field_map: HashMap<i32, Vec<wit::Field>> = HashMap::new();
            if !message.field.is_empty() {
                message.field.iter().for_each(|field| {
                    if field.oneof_index.is_some() && !field.proto3_optional() {
                        if one_of_field_map.contains_key(&field.oneof_index()) {
                            let mut fields =
                                one_of_field_map.get(&field.oneof_index()).unwrap().clone();

                            fields.push(wit::Field::from_util(
                                field,
                                package_name,
                                &format!(".{}.{}", package_name, message.name()),
                                lazy_resources,
                            ));

                            one_of_field_map.insert(field.oneof_index(), fields);
                        } else {
                            let fields = vec![wit::Field::from_util(
                                field,
                                package_name,
                                &format!(".{}.{}", package_name, message.name()),
                                lazy_resources,
                            )];
                            one_of_field_map.insert(field.oneof_index(), fields);
                        }
                    };
                });
            }

            message
                .oneof_decl
                .iter()
                .enumerate()
                .for_each(|(index, oneof)| {
                    let oneof_ = oneof.clone();
                    // oneof_.name = Some();
                    let one_of_type = <wit::Type as OneofFromUtils>::from_util(
                        &oneof_.clone(),
                        &format!("{}.{}", name, oneof.name()),
                    );

                    if one_of_field_map.contains_key(&(index as i32)) {
                        let mut fields = one_of_type.fields.clone();
                        fields.extend(one_of_field_map.get(&(index as i32)).unwrap().clone());

                        types.push(wit::Type {
                            name: format!("{}", one_of_type.name.clone())
                                .to_custom_kebab_case()
                                .check_is_wit_keyword(),
                            is_enum: one_of_type.is_enum,
                            is_record: one_of_type.is_record,
                            is_variant: one_of_type.is_variant,
                            kind: one_of_type.kind.clone(),
                            docs: one_of_type.docs.clone(),
                            fields,
                        })
                    } else {
                        // types.push(one_of_type);
                    }
                });
        }

        types.push(<wit::Type as MessageFromUtils>::from_util(
            message,
            package_name,
            name,
            lazy_resources,
        ));
    }
}

impl MessageFromUtils for wit::Type {
    fn from_util(
        message: &prost_types::DescriptorProto,
        package_name: &str,
        name: &str,
        lazy_resources: &mut Vec<wit::Resource>,
    ) -> Self {
        let mut fields = vec![];

        message.extension.iter().for_each(|extension| {
            fields.push(wit::Field::from_util(
                extension,
                package_name,
                &format!(".{}.{}", package_name, message.name()),
                lazy_resources,
            ));
        });
        message.field.iter().for_each(|field| {
            // if field.type_name() !=  {
            fields.push(wit::Field::from_util(
                field,
                package_name,
                &format!(".{}.{}", package_name, message.name()),
                lazy_resources,
            ));
        });

        // message.oneof_decl.iter().for_each(|oneof| {
        //     // add message name to field name
        //     let field_type_name = format!("{}*{}", message.name(), oneof.name());
        //     fields.push(wit::Field {
        //         full_type_name: "".to_owned(),
        //         name: oneof.name().to_custom_kebab_case().check_is_wit_keyword(),
        //         type_name: field_type_name.to_custom_kebab_case().check_is_wit_keyword(),
        //     });
        // });

        // wit syntax not accepting empty records
        if fields.is_empty() {
            fields.push(wit::Field {
                full_type_name: "".to_owned(),
                name: "empty".to_string(),
                type_name: "bool".to_string(),
            })
        }

        Self {
            name: format!("{}", name)
                .to_custom_kebab_case()
                .check_is_wit_keyword(),
            docs: name.to_owned().replace("*", "."),
            kind: "record".to_owned(),
            is_record: true,
            fields,
            ..Default::default()
        }
    }
}

impl ResourceFromUtils for wit::Resource {
    fn from_util_non_unary(
        service: &prost_types::ServiceDescriptorProto,
        method: &prost_types::MethodDescriptorProto,
        _packgae_name: &str,
    ) -> Self {
        let send = wit::Function {
            name: "send".to_string(),
            docs: "".to_string(),
            parameters: vec![wit::Parameter {
                full_type_name: method.input_type().to_owned(),
                name: "message".to_string(),
                type_name: method
                    .input_type()
                    // .trim_start_matches(&trim)
                    .to_custom_kebab_case()
                    .check_is_wit_keyword(),
            }],
            result: Some(wit::ReturnTypeEnum::Result(wit::ResultReturnType {
                full_type_name: "".to_owned(),
                ok: "option<bool>".to_owned(),
                err: "rpc-grpc-status".to_owned(),
            })),
        };

        let receive = wit::Function {
            name: "receive".to_string(),
            docs: "".to_string(),
            parameters: vec![],
            result: Some(wit::ReturnTypeEnum::Result(wit::ResultReturnType {
                full_type_name: method.output_type().to_owned(),
                ok: format!(
                    "option<{}>",
                    method
                        .output_type()
                        // .trim_start_matches(&trim)
                        .to_custom_kebab_case()
                        .check_is_wit_keyword()
                ),
                err: "rpc-grpc-status".to_owned(),
            })),
        };

        let finish = wit::Function {
            name: "finish".to_string(),
            docs: "".to_string(),
            parameters: vec![],
            result: Some(wit::ReturnTypeEnum::Result(wit::ResultReturnType {
                full_type_name: "".to_owned(),

                ok: "bool".to_string(),
                err: "rpc-grpc-status".to_owned(),
            })),
        };

        let finish_client_streaming = wit::Function {
            name: "finish".to_string(),
            docs: "".to_string(),
            parameters: vec![],
            result: Some(wit::ReturnTypeEnum::Result(wit::ResultReturnType {
                full_type_name: method.output_type().to_owned(),

                ok: method
                    .output_type()
                    // .trim_start_matches(&trim)
                    .to_custom_kebab_case()
                    .check_is_wit_keyword(),
                err: "rpc-grpc-status".to_owned(),
            })),
        };

        let (functions, rpc_type) = match method.client_streaming() && method.server_streaming() {
            true => (vec![send, receive, finish], "bidirectional-streaming"),
            false => match method.server_streaming() {
                true => (vec![send, receive, finish], "server-streaming"),
                false => match method.client_streaming() {
                    true => (vec![send, finish_client_streaming], "client-streaming"),
                    false => (vec![], "unary"),
                },
            },
        };

        let constructor_params = vec![wit::Parameter {
            full_type_name: "".to_owned(),
            name: "grpc-configuration".to_owned(),
            type_name: "rpc-grpc-configuration".to_owned(),
        }];

        let resource = Self {
            name: method.name().to_custom_kebab_case() + &format!("-resource-{}", rpc_type),
            constructor: wit::Constructor {
                name: "new".to_owned(),
                parameters: constructor_params,
            },
            functions,
            docs: service.name().to_owned(),

            ..Default::default()
        };
        resource
    }

    fn from_util(
        service: &prost_types::ServiceDescriptorProto,
        unary_methods: Vec<&prost_types::MethodDescriptorProto>,
        _packgae_name: &str,
    ) -> Self {
        let resource = Self {
            name: service.name().to_custom_kebab_case() + "-resource-unary",
            constructor: wit::Constructor {
                name: "new".to_owned(),
                parameters: vec![wit::Parameter {
                    full_type_name: "".to_owned(),
                    name: "grpc-configuration".to_owned(),
                    type_name: "rpc-grpc-configuration".to_owned(),
                }],
            },
            functions: unary_methods
                .iter()
                .map(|method| {
                    wit::Function {
                        name: method.name().to_custom_kebab_case(),
                        docs: method.name().to_owned(),
                        parameters: vec![wit::Parameter {
                            full_type_name: method.input_type().to_owned(),
                            name: method
                                .input_type()
                                // .trim_start_matches(&trim)
                                .to_custom_kebab_case()
                                .check_is_wit_keyword(),
                            type_name: method
                                .input_type()
                                // .trim_start_matches(&trim)
                                .to_custom_kebab_case()
                                .check_is_wit_keyword(),
                        }],
                        result: Some(wit::ReturnTypeEnum::Result(wit::ResultReturnType {
                            full_type_name: method.output_type().to_owned(),
                            ok: method
                                .output_type()
                                // .trim_start_matches(&trim)
                                .to_custom_kebab_case()
                                .check_is_wit_keyword(),
                            err: "rpc-grpc-status".to_owned(),
                        })),
                    }
                })
                .collect(),
            ..Default::default()
        };
        resource
    }
}

fn sanitize_type_name(type_name: &str) -> String {
    let re1 = regex::Regex::new(r"[^a-zA-Z0-9]").unwrap();
    let mut typ_ = re1.replace_all(type_name, "-").into_owned();

    typ_ = typ_.trim_start_matches("-").to_owned();
    typ_
}

fn sanitize_for_kebab(type_name: &str) -> String {
    // find any two Capital sitting side by side then place - infront of first
    // GRPCBin -> G-R-P-C-Bin
    // repeact regex replace untill not found anymore.
    let mut typ_ = type_name.to_owned();
    let re = regex::Regex::new(r"([A-Z])([A-Z])").unwrap();
    loop {
        let old_typ_ = typ_.clone();
        typ_ = re.replace_all(&typ_, "$1-$2").into_owned();
        if old_typ_ == typ_ {
            break;
        }
    }
    typ_
}

fn sanitize_package_version(version: &str) -> String {
    let mut package_version = version.to_owned();
    package_version = package_version.trim_start_matches("v").to_owned();
    let parts = package_version.split("_").collect::<Vec<&str>>();
    // make sure parts are numbers
    for part in parts.clone() {
        if !part.chars().all(|c| c.is_numeric()) {
            return "0.1.0".to_owned();
        }
    }
    if parts.is_empty() {
        return "0.1.0".to_owned();
    }
    if parts.len() == 1 {
        format!("{}.0.0", parts[0])
    } else if parts.len() == 2 {
        return format!("{}.{}.0", parts[0], parts[1]);
    } else {
        // for >=3
        return format!("{}.{}.{}", parts[0], parts[1], parts[2]);
    }
}

fn _process_use_type(type_: &str, uses: &mut HashMap<String, HashSet<String>>) {
    // find index of . from last
    let index = type_.rfind('.');
    if index.is_some() {
        let split = type_.split_at(index.unwrap());
        uses.entry(split.0.to_custom_kebab_case())
            .or_default()
            .insert(split.1.to_custom_kebab_case());
    }
}

pub trait ToCustomKebabCase {
    fn to_custom_kebab_case(&self) -> String;
    fn check_is_wit_keyword(&self) -> String;
}

impl ToCustomKebabCase for str {
    fn to_custom_kebab_case(&self) -> String {
        sanitize_for_kebab(self).to_kebab_case()
    }
    fn check_is_wit_keyword(&self) -> String {
        if matches!(
            self,
            "as" | "async"
                | "bool"
                | "borrow"
                | "char"
                | "constructor"
                | "enum"
                | "export"
                | "f32"
                | "f64"
                | "flags"
                | "from"
                | "func"
                | "future"
                | "import"
                | "include"
                | "interface"
                | "list"
                | "option"
                | "own"
                | "package"
                | "record"
                | "resource"
                | "result"
                | "s16"
                | "s32"
                | "s64"
                | "s8"
                | "static"
                | "stream"
                | "string"
                | "tuple"
                | "type"
                | "u16"
                | "u32"
                | "u64"
                | "u8"
                | "use"
                | "variant"
                | "with"
                | "world"
        ) {
            format!("%{}", self)
        } else {
            self.to_string()
        }
    }
}

#[cfg(test)]
mod tests {

    use crate::{from_grpc, WitUtils};
    use std::{fs, path::Path};

    #[test]
    fn it_works() {
        let (wit, package_name, _) = from_grpc(&[Path::new("/home/balaram/golem-fork/test-components/grpc-example1_app/components-python/rpc-grpc-texttospeech-app/googleapis/google/cloud/tpu/v1/cloud_tpu.proto")], &[Path::new("/home/balaram/golem-fork/test-components/grpc-example1_app/components-python/rpc-grpc-texttospeech-app/googleapis/")],  None);

        // let (wit, package_name, _) = from_grpc(&[Path::new("/home/balaram/golem-fork/wc/in/index.proto")], &[Path::new("/home/balaram/golem-fork/wc/in/")],  None);

        // let (wit, package_name, _) = from_grpc(&[Path::new("/home/balaram/golem-fork/test-components/grpc-example1_app/components-rust/rpc-grpc-api-caller/proto/index.proto")], &[Path::new("/home/balaram/golem-fork/test-components/grpc-example1_app/components-rust/rpc-grpc-api-caller/proto")],  None);

        fs::write("out/wit.wit", wit.to_string_format()).unwrap();
        println!(
            "WIT file generated successfully for package {}",
            package_name
        );
        assert_eq!(0, 0);
    }
}
