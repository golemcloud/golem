pub mod wit {
    use crate::log::LogColorize;
    use crate::wasm_rpc_stubgen::stub::FunctionStub;
    use anyhow::{anyhow, bail};
    use std::path::{Path, PathBuf};

    pub static DEPS_DIR: &str = "deps";
    pub static WIT_DIR: &str = "wit";

    pub static CLIENT_WIT_FILE_NAME: &str = "client.wit";
    pub static EXPORTS_WIT_FILE_NAME: &str = "exports.wit";

    pub fn client_parser_package_name(
        package_name: &wit_parser::PackageName,
    ) -> wit_parser::PackageName {
        wit_parser::PackageName {
            namespace: package_name.namespace.clone(),
            name: format!("{}-client", package_name.name),
            version: package_name.version.clone(),
        }
    }

    pub fn client_encoder_package_name(
        package_name: &wit_parser::PackageName,
    ) -> wit_encoder::PackageName {
        wit_encoder::PackageName::new(
            package_name.namespace.clone(),
            format!("{}-client", package_name.name),
            package_name.version.clone(),
        )
    }

    pub fn client_interface_name(source_world: &wit_parser::World) -> String {
        format!("{}-client", source_world.name)
    }

    pub fn exports_parser_package_name(
        package_name: &wit_parser::PackageName,
    ) -> wit_parser::PackageName {
        wit_parser::PackageName {
            namespace: package_name.namespace.clone(),
            name: format!("{}-exports", package_name.name),
            version: package_name.version.clone(),
        }
    }

    pub fn exports_encoder_package_name(
        package_name: &wit_encoder::PackageName,
    ) -> wit_encoder::PackageName {
        wit_encoder::PackageName::new(
            package_name.namespace(),
            format!("{}-exports", package_name.name()),
            package_name.version().cloned(),
        )
    }

    pub fn exports_package_world_inline_interface_name(
        world_name: &wit_encoder::Ident,
        interface_name: &wit_encoder::Ident,
    ) -> String {
        format!("{}-{}", world_name.raw_name(), interface_name.raw_name())
    }

    pub fn interface_package_world_inline_functions_interface_name(
        world_name: &wit_encoder::Ident,
    ) -> String {
        format!("{}-inline-functions", world_name.raw_name())
    }

    pub fn client_target_package_name(
        client_package_name: &wit_parser::PackageName,
    ) -> wit_parser::PackageName {
        wit_parser::PackageName {
            namespace: client_package_name.namespace.clone(),
            name: client_package_name
                .name
                .strip_suffix("-client")
                .expect("Unexpected client package name")
                .to_string(),
            version: client_package_name.version.clone(),
        }
    }

    pub fn client_import_name(client_package: &wit_parser::Package) -> anyhow::Result<String> {
        let package_name = &client_package.name;

        if client_package.interfaces.len() != 1 {
            bail!(
                "Expected exactly one interface in client package, package name: {}",
                package_name.to_string().log_color_highlight()
            );
        }

        let interface_name = client_package.interfaces.first().unwrap().0;

        Ok(format!(
            "{}:{}/{}{}",
            package_name.namespace,
            package_name.name,
            interface_name,
            package_name
                .version
                .as_ref()
                .map(|version| format!("@{version}"))
                .unwrap_or_default()
        ))
    }

    pub fn client_import_exports_prefix_from_client_package_name(
        client_package: &wit_parser::PackageName,
    ) -> anyhow::Result<String> {
        Ok(format!(
            "{}:{}-exports/",
            client_package.namespace,
            client_package
                .name
                .clone()
                .strip_suffix("-client")
                .ok_or_else(|| anyhow!(
                    "Expected \"-client\" suffix in client package name: {}",
                    client_package.to_string()
                ))?
        ))
    }

    pub fn package_dep_dir_name_from_parser(package_name: &wit_parser::PackageName) -> String {
        format!("{}_{}", package_name.namespace, package_name.name)
    }

    pub fn package_dep_dir_name_from_encoder(package_name: &wit_encoder::PackageName) -> String {
        format!("{}_{}", package_name.namespace(), package_name.name())
    }

    pub fn package_merged_wit_name(package_name: &wit_parser::PackageName) -> String {
        format!("{}_{}.wit", package_name.namespace, package_name.name)
    }

    pub fn package_wit_dep_dir_from_package_dir_name(package_dir_name: &str) -> PathBuf {
        Path::new(WIT_DIR).join(DEPS_DIR).join(package_dir_name)
    }

    pub fn package_wit_dep_dir_from_parser(package_name: &wit_parser::PackageName) -> PathBuf {
        package_wit_dep_dir_from_package_dir_name(&package_dep_dir_name_from_parser(package_name))
    }

    pub fn package_wit_dep_dir_from_encode(package_name: &wit_encoder::PackageName) -> PathBuf {
        package_wit_dep_dir_from_package_dir_name(&package_dep_dir_name_from_encoder(package_name))
    }

    pub fn blocking_function_name(function: &FunctionStub) -> String {
        format!("blocking-{}", function.name)
    }

    pub fn schedule_function_name(function: &FunctionStub) -> String {
        format!("schedule-{}", function.name)
    }
}

pub mod rust {
    use crate::wasm_rpc_stubgen::stub::{FunctionStub, StubbedEntity};
    use heck::{ToSnakeCase, ToUpperCamelCase};
    use proc_macro2::{Ident, Span};
    use wit_bindgen_rust::to_rust_ident;

    pub static CARGO_TOML: &str = "Cargo.toml";
    pub static SRC: &str = "src/lib.rs";

    pub fn root_namespace(source_package_name: &wit_parser::PackageName) -> String {
        source_package_name.namespace.to_snake_case()
    }

    pub fn client_root_name(source_package_name: &wit_parser::PackageName) -> String {
        format!("{}_client", source_package_name.name.to_snake_case())
    }

    pub fn client_crate_name(source_world: &wit_parser::World) -> String {
        format!("{}-client", source_world.name)
    }

    pub fn client_interface_name(source_world: &wit_parser::World) -> String {
        to_rust_ident(&format!("{}-client", source_world.name)).to_snake_case()
    }

    pub fn client_world_name(source_world: &wit_parser::World) -> String {
        format!("wasm-rpc-client-{}", source_world.name)
    }

    pub fn result_wrapper_ident(function: &FunctionStub, owner: &StubbedEntity) -> Ident {
        Ident::new(
            &to_rust_ident(&function.async_result_type(owner)).to_upper_camel_case(),
            Span::call_site(),
        )
    }

    pub fn result_wrapper_interface_ident(function: &FunctionStub, owner: &StubbedEntity) -> Ident {
        Ident::new(
            &to_rust_ident(&format!("guest-{}", function.async_result_type(owner)))
                .to_upper_camel_case(),
            Span::call_site(),
        )
    }
}
