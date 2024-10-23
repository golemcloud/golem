use anyhow::anyhow;
use indexmap::IndexMap;
use regex::Regex;
use wit_encoder::{
    packages_from_parsed, Enum, Flags, Ident, Include, Interface, InterfaceItem, Package,
    PackageItem, PackageName, Record, Resource, StandaloneFunc, Type, TypeDef, TypeDefKind, Use,
    Variant, World, WorldItem, WorldNamedInterface,
};

// TODO: add skip option?
pub trait VisitPackage {
    #[allow(unused_variables)]
    fn package(&mut self, package: &mut Package) {}

    #[allow(unused_variables)]
    fn package_interface(&mut self, package_name: &PackageName, interface: &mut Interface) {}

    #[allow(unused_variables)]
    fn package_interface_use(
        &mut self,
        package_name: &PackageName,
        interface_name: &Ident,
        use_: &mut Use,
    ) {
    }

    #[allow(unused_variables)]
    fn package_interface_type_def(
        &mut self,
        package_name: &PackageName,
        interface_name: &Ident,
        type_def: &mut TypeDef,
    ) {
    }

    #[allow(unused_variables)]
    fn package_interface_type_def_record(
        &mut self,
        package_name: &PackageName,
        interface_name: &Ident,
        type_def_name: &Ident,
        record: &mut Record,
    ) {
    }

    #[allow(unused_variables)]
    fn package_interface_type_def_resource(
        &mut self,
        package_name: &PackageName,
        interface_name: &Ident,
        type_def_name: &Ident,
        resource: &mut Resource,
    ) {
    }

    #[allow(unused_variables)]
    fn package_interface_type_def_flags(
        &mut self,
        package_name: &PackageName,
        interface_name: &Ident,
        type_def_name: &Ident,
        flags: &mut Flags,
    ) {
    }

    #[allow(unused_variables)]
    fn package_interface_type_def_variant(
        &mut self,
        package_name: &PackageName,
        interface_name: &Ident,
        type_def_name: &Ident,
        variant: &mut Variant,
    ) {
    }

    #[allow(unused_variables)]
    fn package_interface_type_def_enum(
        &mut self,
        package_name: &PackageName,
        interface_name: &Ident,
        type_def_name: &Ident,
        enum_: &mut Enum,
    ) {
    }

    #[allow(unused_variables)]
    fn package_interface_type_def_type(
        &mut self,
        package_name: &PackageName,
        interface_name: &Ident,
        type_def_name: &Ident,
        type_: &mut Type,
    ) {
    }

    #[allow(unused_variables)]
    fn package_interface_function(
        &mut self,
        package_name: &PackageName,
        interface_name: &Ident,
        function: &mut StandaloneFunc,
    ) {
    }

    #[allow(unused_variables)]
    fn package_world(&mut self, package_name: &PackageName, world: &mut World) {}

    #[allow(unused_variables)]
    fn package_world_inline_interface_import(
        &mut self,
        package_name: &PackageName,
        world_name: &Ident,
        inline_interface_import: &mut Interface,
    ) {
    }

    #[allow(unused_variables)]
    fn package_world_inline_interface_export(
        &mut self,
        package_name: &PackageName,
        world_name: &Ident,
        inline_interface_export: &mut Interface,
    ) {
    }

    #[allow(unused_variables)]
    fn package_world_named_interface_import(
        &mut self,
        package_name: &PackageName,
        world_name: &Ident,
        named_interface_import: &mut WorldNamedInterface,
    ) {
    }

    #[allow(unused_variables)]
    fn package_world_named_interface_export(
        &mut self,
        package_name: &PackageName,
        world_name: &Ident,
        named_interface_export: &mut WorldNamedInterface,
    ) {
    }

    #[allow(unused_variables)]
    fn package_world_function_import(
        &mut self,
        package_name: &PackageName,
        world_name: &Ident,
        function_import: &mut StandaloneFunc,
    ) {
    }

    #[allow(unused_variables)]
    fn package_world_function_export(
        &mut self,
        package_name: &PackageName,
        world_name: &Ident,
        function_export: &mut StandaloneFunc,
    ) {
    }

    #[allow(unused_variables)]
    fn package_world_include(
        &mut self,
        package_name: &PackageName,
        world_name: &Ident,
        include: &mut Include,
    ) {
    }

    #[allow(unused_variables)]
    fn package_world_use(
        &mut self,
        package_name: &PackageName,
        world_name: &Ident,
        use_: &mut Use,
    ) {
    }
}

pub fn visit_package(package: &mut Package, visitor: &mut impl VisitPackage) {
    visitor.package(package);
    let package_name = package.name().clone();

    for item in package.items_mut() {
        match item {
            PackageItem::Interface(interface) => {
                visitor.package_interface(&package_name, interface);

                let interface_name = interface.name().clone();

                for use_ in interface.uses_mut() {
                    visitor.package_interface_use(&package_name, &interface_name, use_);
                }

                for item in interface.items_mut() {
                    match item {
                        InterfaceItem::TypeDef(type_def) => {
                            visitor.package_interface_type_def(
                                &package_name,
                                &interface_name,
                                type_def,
                            );

                            let type_def_name = type_def.name().clone();

                            match type_def.kind_mut() {
                                TypeDefKind::Record(record) => {
                                    visitor.package_interface_type_def_record(
                                        &package_name,
                                        &interface_name,
                                        &type_def_name,
                                        record,
                                    );
                                }
                                TypeDefKind::Resource(resource) => {
                                    visitor.package_interface_type_def_resource(
                                        &package_name,
                                        &interface_name,
                                        &type_def_name,
                                        resource,
                                    );
                                }
                                TypeDefKind::Flags(flags) => {
                                    visitor.package_interface_type_def_flags(
                                        &package_name,
                                        &interface_name,
                                        &type_def_name,
                                        flags,
                                    );
                                }
                                TypeDefKind::Variant(variant) => {
                                    visitor.package_interface_type_def_variant(
                                        &package_name,
                                        &interface_name,
                                        &type_def_name,
                                        variant,
                                    );
                                }
                                TypeDefKind::Enum(enum_) => {
                                    visitor.package_interface_type_def_enum(
                                        &package_name,
                                        &interface_name,
                                        &type_def_name,
                                        enum_,
                                    );
                                }
                                TypeDefKind::Type(type_) => {
                                    visitor.package_interface_type_def_type(
                                        &package_name,
                                        &interface_name,
                                        &type_def_name,
                                        type_,
                                    );
                                }
                            }
                        }
                        InterfaceItem::Function(function) => {
                            visitor.package_interface_function(
                                &package_name,
                                &interface_name,
                                function,
                            );
                        }
                    }
                }
            }
            PackageItem::World(world) => {
                visitor.package_world(&package_name, world);

                let world_name = world.name().clone();

                for item in world.items_mut() {
                    match item {
                        WorldItem::InlineInterfaceImport(inline_interface_import) => {
                            visitor.package_world_inline_interface_import(
                                &package_name,
                                &world_name,
                                inline_interface_import,
                            );
                        }
                        WorldItem::InlineInterfaceExport(inline_interface_export) => {
                            visitor.package_world_inline_interface_export(
                                &package_name,
                                &world_name,
                                inline_interface_export,
                            );
                        }
                        WorldItem::NamedInterfaceImport(named_interface_import) => {
                            visitor.package_world_named_interface_import(
                                &package_name,
                                &world_name,
                                named_interface_import,
                            );
                        }
                        WorldItem::NamedInterfaceExport(named_interface_export) => {
                            visitor.package_world_named_interface_export(
                                &package_name,
                                &world_name,
                                named_interface_export,
                            );
                        }
                        WorldItem::FunctionImport(function_import) => {
                            visitor.package_world_function_import(
                                &package_name,
                                &world_name,
                                function_import,
                            );
                        }
                        WorldItem::FunctionExport(function_export) => {
                            visitor.package_world_function_export(
                                &package_name,
                                &world_name,
                                function_export,
                            );
                        }
                        WorldItem::Include(include) => {
                            visitor.package_world_include(&package_name, &world_name, include);
                        }
                        WorldItem::Use(use_) => {
                            visitor.package_world_use(&package_name, &world_name, use_);
                        }
                    }
                }
            }
        }
    }
}

pub fn import_remover(
    package_name: &wit_parser::PackageName,
) -> impl Fn(String) -> anyhow::Result<String> {
    let pattern_import_stub_package_name = Regex::new(
        format!(
            r"import\s+{}(/[^;]*)?;",
            regex::escape(&package_name.to_string())
        )
        .as_str(),
    )
    .unwrap_or_else(|err| panic!("Failed to compile package import regex: {}", err));

    move |src: String| -> anyhow::Result<String> {
        Ok(pattern_import_stub_package_name
            .replace_all(&src, "")
            .to_string())
    }
}

pub struct WitTransformer<'a> {
    _resolve: &'a wit_parser::Resolve, // TODO
    encoded_packages_by_parser_id: IndexMap<wit_parser::PackageId, Package>,
}

impl<'a> WitTransformer<'a> {
    pub fn new(resolve: &'a wit_parser::Resolve) -> anyhow::Result<Self> {
        let mut encoded_packages_by_parser_id = IndexMap::<wit_parser::PackageId, Package>::new();

        for package in packages_from_parsed(resolve) {
            let package_name = package.name();
            let package_name = wit_parser::PackageName {
                namespace: package_name.namespace().to_string(),
                name: package_name.name().to_string(),
                version: package_name.version().cloned(),
            };

            let package_id = resolve
                .package_names
                .get(&package_name)
                .cloned()
                .ok_or_else(|| anyhow!("Failed to get package by name: {}", package.name()))?;
            encoded_packages_by_parser_id.insert(package_id, package);
        }

        Ok(Self {
            _resolve: resolve,
            encoded_packages_by_parser_id,
        })
    }

    pub fn render_package(&mut self, package_id: wit_parser::PackageId) -> anyhow::Result<String> {
        Ok(format!("{}", self.encoded_package(package_id)?))
    }

    fn encoded_package(
        &mut self,
        package_id: wit_parser::PackageId,
    ) -> anyhow::Result<&mut Package> {
        self.encoded_packages_by_parser_id
            .get_mut(&package_id)
            .ok_or_else(|| anyhow!("Failed to get encoded package by id: {:?}", package_id))
    }

    pub fn remove_imports_from_package_all_worlds(
        &mut self,
        package_id: wit_parser::PackageId,
        import_prefix: &str,
    ) -> anyhow::Result<()> {
        struct RemoveImports<'a> {
            import_prefix: &'a str,
        }
        impl<'a> VisitPackage for RemoveImports<'a> {
            fn package_world(&mut self, _package_name: &PackageName, world: &mut World) {
                world.items_mut().retain(|item| {
                    if let WorldItem::NamedInterfaceImport(import) = &item {
                        !import.name().raw_name().starts_with(self.import_prefix)
                    } else {
                        true
                    }
                });
            }
        }

        visit_package(
            self.encoded_package(package_id)?,
            &mut RemoveImports { import_prefix },
        );

        Ok(())
    }
}

pub fn add_import_to_world(package: &mut Package, world_name: Ident, import_name: Ident) {
    struct AddImportToWorld {
        world_name: Ident,
        import_name: Ident,
    }
    impl VisitPackage for AddImportToWorld {
        fn package_world(&mut self, _package_name: &PackageName, world: &mut World) {
            if *world.name() == self.world_name {
                let is_already_imported = world.items_mut().iter().any(|item| {
                    if let WorldItem::NamedInterfaceImport(import) = item {
                        *import.name() == self.import_name
                    } else {
                        false
                    }
                });
                if !is_already_imported {
                    world.named_interface_import(self.import_name.clone());
                }
            }
        }
    }

    visit_package(
        package,
        &mut AddImportToWorld {
            world_name,
            import_name,
        },
    );
}
