pub mod wit {
    use crate::log::LogColorize;
    use anyhow::{anyhow, bail};
    use std::path::{Path, PathBuf};

    pub static DEPS_DIR: &str = "deps";
    pub static WIT_DIR: &str = "wit";

    pub static STUB_WIT_FILE_NAME: &str = "stub.wit";
    pub static INTERFACE_WIT_FILE_NAME: &str = "interface.wit";

    pub fn stub_package_name(package_name: &wit_parser::PackageName) -> wit_parser::PackageName {
        wit_parser::PackageName {
            namespace: package_name.namespace.clone(),
            name: format!("{}-stub", package_name.name),
            version: package_name.version.clone(),
        }
    }

    pub fn interface_parser_package_name(
        package_name: &wit_parser::PackageName,
    ) -> wit_parser::PackageName {
        wit_parser::PackageName {
            namespace: package_name.namespace.clone(),
            name: format!("{}-interface", package_name.name),
            version: package_name.version.clone(),
        }
    }

    pub fn interface_encoder_package_name(
        package_name: &wit_encoder::PackageName,
    ) -> wit_encoder::PackageName {
        wit_encoder::PackageName::new(
            package_name.namespace(),
            format!("{}-interface", package_name.name()),
            package_name.version().cloned(),
        )
    }

    pub fn interface_package_world_inline_interface_name(
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

    pub fn stub_target_package_name(
        stub_package_name: &wit_parser::PackageName,
    ) -> wit_parser::PackageName {
        wit_parser::PackageName {
            namespace: stub_package_name.namespace.clone(),
            name: stub_package_name
                .name
                .strip_suffix("-stub")
                .expect("Unexpected stub package name")
                .to_string(),
            version: stub_package_name.version.clone(),
        }
    }

    pub fn stub_import_name(stub_package: &wit_parser::Package) -> anyhow::Result<String> {
        let package_name = &stub_package.name;

        if stub_package.interfaces.len() != 1 {
            bail!(
                "Expected exactly one interface in stub package, package name: {}",
                package_name.to_string().log_color_highlight()
            );
        }

        let interface_name = stub_package.interfaces.first().unwrap().0;

        Ok(format!(
            "{}:{}/{}{}",
            package_name.namespace,
            package_name.name,
            interface_name,
            package_name
                .version
                .as_ref()
                .map(|version| format!("@{}", version))
                .unwrap_or_default()
        ))
    }

    pub fn stub_import_interface_prefix_from_stub_package_name(
        stub_package: &wit_parser::PackageName,
    ) -> anyhow::Result<String> {
        Ok(format!(
            "{}:{}-interface/",
            stub_package.namespace,
            stub_package
                .name
                .clone()
                .strip_suffix("-stub")
                .ok_or_else(|| anyhow!(
                    "Expected \"-stub\" suffix in stub package name: {}",
                    stub_package.to_string()
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
}
