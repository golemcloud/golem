pub mod wit {
    use wit_parser::PackageName;

    pub static DEPS_DIR: &str = "deps";
    pub static WIT_DIR: &str = "wit";

    pub static STUB_WIT_FILE_NAME: &str = "_stub.wit";

    pub fn stub_package_name(package_name: &PackageName) -> PackageName {
        PackageName {
            namespace: package_name.namespace.clone(),
            name: format!("{}-stub", package_name.name),
            version: package_name.version.clone(),
        }
    }

    pub fn stub_target_package_name(stub_package_name: &PackageName) -> PackageName {
        PackageName {
            namespace: stub_package_name.namespace.clone(),
            name: stub_package_name
                .name
                .strip_suffix("-stub")
                .expect("Unexpected stub package name")
                .to_string(),
            version: stub_package_name.version.clone(),
        }
    }

    pub fn package_dep_folder_name(package_name: &PackageName) -> String {
        format!("{}_{}", package_name.namespace, package_name.name)
    }
}
