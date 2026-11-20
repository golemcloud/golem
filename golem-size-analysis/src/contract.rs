use anyhow::{Context, bail, ensure};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use wit_parser::{Resolve, WorldId};

/// The five guest interfaces every Golem SDK component exports.
pub const GOLEM_GUEST_EXPORTS: [&str; 5] = [
    "golem:agent/guest@2.0.0",
    "golem:tool/guest@0.1.0",
    "golem:tool/tool-middleware-guest@0.1.0",
    "golem:api/load-snapshot@1.5.0",
    "golem:api/save-snapshot@1.5.0",
];

/// The named types (by structural hash) and function signatures of one interface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterfaceContract {
    pub types: BTreeMap<String, String>,
    pub functions: BTreeMap<String, Value>,
}

/// The interfaces a component's world exports and imports, keyed by `package/interface@version`.
/// Types are compared by structure, so declaration order, type indices, aliases and docs do not
/// change the contract.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorldContract {
    pub exports: BTreeMap<String, InterfaceContract>,
    pub imports: BTreeMap<String, InterfaceContract>,
}

/// The contract of an encoded component.
pub fn component_contract(component: &[u8]) -> anyhow::Result<WorldContract> {
    match wit_parser::decoding::decode(component).context("could not decode the component's WIT")? {
        wit_parser::decoding::DecodedWasm::Component(resolve, world) => contract(&resolve, world),
        wit_parser::decoding::DecodedWasm::WitPackage(..) => {
            bail!("expected a component, found a WIT package")
        }
    }
}

/// The contract of `world`, computed on the same JSON form `wasm-tools component wit --json`
/// prints.
pub fn contract(resolve: &Resolve, world: WorldId) -> anyhow::Result<WorldContract> {
    let wit = serde_json::to_value(resolve)?;
    let mut hasher = TypeHasher {
        wit: &wit,
        cache: HashMap::new(),
    };
    let package = resolve.worlds[world]
        .package
        .context("world has no package")?;
    ensure!(
        resolve.packages[package].worlds.len() == 1,
        "expected one world in the component's package"
    );
    let world = &wit["worlds"][world.index()];
    Ok(WorldContract {
        exports: hasher.interfaces(&world["exports"])?,
        imports: hasher.interfaces(&world["imports"])?,
    })
}

struct TypeHasher<'a> {
    wit: &'a Value,
    cache: HashMap<u64, Value>,
}

impl TypeHasher<'_> {
    fn interface_name(&self, index: &Value) -> anyhow::Result<String> {
        let interface =
            &self.wit["interfaces"][index.as_u64().context("interface index")? as usize];
        let package = self.wit["packages"]
            [interface["package"].as_u64().context("package")? as usize]["name"]
            .as_str()
            .context("package name")?;
        let name = interface["name"].as_str().context("interface name")?;
        Ok(match package.split_once('@') {
            Some((package, version)) => format!("{package}/{name}@{version}"),
            None => format!("{package}/{name}"),
        })
    }

    fn type_hash(&mut self, index: u64) -> anyhow::Result<Value> {
        if let Some(hash) = self.cache.get(&index) {
            return Ok(hash.clone());
        }
        let ty = &self.wit["types"][index as usize];
        let kind = &ty["kind"];
        let hash = if kind == "resource" {
            let owner = self.interface_name(&ty["owner"]["interface"])?;
            Value::String(format!(
                "{owner}/{}",
                ty["name"].as_str().unwrap_or_default()
            ))
        } else if let Some(alias) = kind
            .as_object()
            .filter(|k| k.len() == 1)
            .and_then(|k| k.get("type"))
        {
            // An alias does not change the identity of the type it names.
            self.normalize(alias)?
        } else {
            let normalized = self.normalize(kind)?;
            Value::String(hex::encode(Sha256::digest(canonical(&normalized))))
        };
        self.cache.insert(index, hash.clone());
        Ok(hash)
    }

    fn normalize(&mut self, value: &Value) -> anyhow::Result<Value> {
        Ok(match value {
            Value::Number(n) if n.is_u64() => self.type_hash(n.as_u64().unwrap_or_default())?,
            Value::Array(items) => Value::Array(
                items
                    .iter()
                    .map(|item| self.normalize(item))
                    .collect::<anyhow::Result<_>>()?,
            ),
            Value::Object(fields) => Value::Object(
                fields
                    .iter()
                    .filter(|(key, _)| !matches!(key.as_str(), "docs" | "stability"))
                    .map(|(key, item)| Ok((key.clone(), self.normalize(item)?)))
                    .collect::<anyhow::Result<_>>()?,
            ),
            other => other.clone(),
        })
    }

    fn interfaces(&mut self, items: &Value) -> anyhow::Result<BTreeMap<String, InterfaceContract>> {
        let mut result = BTreeMap::new();
        for item in items.as_object().context("world items")?.values() {
            // Golem worlds export and import only interfaces, so anything else would escape the
            // comparison.
            let id = &item
                .get("interface")
                .with_context(|| format!("world item is not an interface: {item}"))?["id"];
            let interface = &self.wit["interfaces"][id.as_u64().context("interface id")? as usize];
            let mut types = BTreeMap::new();
            for (name, ty) in interface["types"].as_object().context("interface types")? {
                let hash = self.type_hash(ty.as_u64().context("type index")?)?;
                types.insert(name.clone(), canonical_string(&hash));
            }
            let mut functions = BTreeMap::new();
            for (name, function) in interface["functions"]
                .as_object()
                .context("interface functions")?
            {
                functions.insert(name.clone(), self.normalize(function)?);
            }
            result.insert(
                self.interface_name(id)?,
                InterfaceContract { types, functions },
            );
        }
        Ok(result)
    }
}

fn canonical_string(value: &Value) -> String {
    String::from_utf8(canonical(value)).expect("canonical JSON is UTF-8")
}

/// JSON with object keys sorted, independent of how `serde_json` orders maps.
fn canonical(value: &Value) -> Vec<u8> {
    fn write(value: &Value, out: &mut Vec<u8>) {
        match value {
            Value::Object(fields) => {
                out.push(b'{');
                let sorted: BTreeMap<_, _> = fields.iter().collect();
                for (i, (key, item)) in sorted.into_iter().enumerate() {
                    if i > 0 {
                        out.push(b',');
                    }
                    out.extend(serde_json::to_vec(key).expect("string"));
                    out.push(b':');
                    write(item, out);
                }
                out.push(b'}');
            }
            Value::Array(items) => {
                out.push(b'[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(b',');
                    }
                    write(item, out);
                }
                out.push(b']');
            }
            other => out.extend(serde_json::to_vec(other).expect("scalar")),
        }
    }
    let mut out = Vec::new();
    write(value, &mut out);
    out
}

/// Checks that an SDK optimization kept the export contract: `before` and `after` export exactly
/// `expected`, with unchanged types and signatures, and `after` imports nothing new. Imports that
/// `after` dropped (dead imports) are allowed.
pub fn check_export_contract(
    before: &WorldContract,
    after: &WorldContract,
    expected: &[&str],
) -> anyhow::Result<()> {
    let expected: BTreeSet<&str> = expected.iter().copied().collect();
    for (label, contract) in [("before", before), ("after", after)] {
        let exports: BTreeSet<&str> = contract.exports.keys().map(String::as_str).collect();
        ensure!(
            exports == expected,
            "{label} exports {exports:?}, expected {expected:?}"
        );
    }
    for name in &expected {
        ensure!(
            before.exports[*name] == after.exports[*name],
            "changed export: {name}"
        );
    }
    for (name, interface) in &after.imports {
        let original = before
            .imports
            .get(name)
            .with_context(|| format!("new import interface: {name}"))?;
        for (member, hash) in &interface.types {
            ensure!(
                original.types.get(member) == Some(hash),
                "new or changed import: {name}#{member}"
            );
        }
        for (member, signature) in &interface.functions {
            ensure!(
                original.functions.get(member) == Some(signature),
                "new or changed import: {name}#{member}"
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    fn try_world(source: &str) -> anyhow::Result<WorldContract> {
        let mut resolve = Resolve::default();
        let package = resolve.push_source("test.wit", source)?;
        let world = resolve.select_world(&[package], Some("w"))?;
        contract(&resolve, world)
    }

    fn world(source: &str) -> WorldContract {
        try_world(source).unwrap()
    }

    const BEFORE: &str = r#"
        package test:pkg@1.0.0;
        interface host {
            record point { x: u32, y: u32 }
            log: func(message: string);
            dead: func() -> u64;
        }
        interface guest {
            use host.{point};
            variant shape { dot(point), none }
            resource counter { constructor(start: u32); inc: func() -> u32; }
            area: func(s: shape) -> f64;
        }
        world w { import host; export guest; }
    "#;

    #[test]
    fn declaration_order_and_dead_imports_do_not_change_the_contract() {
        let after = world(
            r#"
            package test:pkg@1.0.0;
            interface guest {
                /// Docs do not matter either.
                area: func(s: shape) -> f64;
                resource counter { inc: func() -> u32; constructor(start: u32); }
                variant shape { dot(point), none }
                use host.{point};
            }
            interface host {
                log: func(message: string);
                record point { x: u32, y: u32 }
            }
            world w { export guest; import host; }
        "#,
        );
        check_export_contract(&world(BEFORE), &after, &["test:pkg/guest@1.0.0"]).unwrap();
    }

    #[test]
    fn a_changed_signature_or_new_import_fails() {
        let changed_export =
            world(&BEFORE.replace("area: func(s: shape) -> f64", "area: func(s: shape) -> f32"));
        let error =
            check_export_contract(&world(BEFORE), &changed_export, &["test:pkg/guest@1.0.0"])
                .unwrap_err();
        assert_eq!(error.to_string(), "changed export: test:pkg/guest@1.0.0");

        let changed_type = world(&BEFORE.replace(
            "record point { x: u32, y: u32 }",
            "record point { x: u32, y: s32 }",
        ));
        assert!(
            check_export_contract(&world(BEFORE), &changed_type, &["test:pkg/guest@1.0.0"])
                .is_err()
        );

        let new_import = world(&BEFORE.replace(
            "dead: func() -> u64;",
            "dead: func() -> u64; extra: func();",
        ));
        let error = check_export_contract(&world(BEFORE), &new_import, &["test:pkg/guest@1.0.0"])
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "new or changed import: test:pkg/host@1.0.0#extra"
        );

        let error =
            check_export_contract(&world(BEFORE), &world(BEFORE), &["test:pkg/other@1.0.0"])
                .unwrap_err();
        assert!(error.to_string().starts_with("before exports"), "{error}");
    }

    #[test]
    fn aliases_and_resource_owners_are_part_of_the_contract() {
        let aliased = world(&BEFORE.replace(
            "variant shape { dot(point), none }",
            "type pt = point; variant shape { dot(pt), none }",
        ));
        check_export_contract(&world(BEFORE), &aliased, &["test:pkg/guest@1.0.0"]).unwrap_err();
        let alias_only = world(&BEFORE.replace(
            "area: func(s: shape) -> f64;",
            "area: func(s: shape) -> f64; type pt = point;",
        ));
        // A new alias of an existing type names it, so the interface's type map changes.
        assert_ne!(
            world(BEFORE).exports["test:pkg/guest@1.0.0"].types,
            alias_only.exports["test:pkg/guest@1.0.0"].types
        );
        assert_eq!(
            alias_only.exports["test:pkg/guest@1.0.0"].types["pt"],
            alias_only.exports["test:pkg/guest@1.0.0"].types["point"]
        );
        let moved = world(&BEFORE.replace(
            "resource counter { constructor(start: u32); inc: func() -> u32; }",
            "use host.{counter};",
        ).replace("dead: func() -> u64;", "dead: func() -> u64; resource counter { constructor(start: u32); inc: func() -> u32; }"));
        let error =
            check_export_contract(&world(BEFORE), &moved, &["test:pkg/guest@1.0.0"]).unwrap_err();
        assert!(error.to_string().contains("guest"), "{error}");
    }

    #[test]
    fn world_level_functions_and_extra_worlds_are_rejected() {
        let error = try_world(&BEFORE.replace(
            "world w { import host; export guest; }",
            "world w { import host; export guest; export run: func(); }",
        ))
        .unwrap_err();
        assert!(error.to_string().contains("not an interface"), "{error}");
        let error = try_world(&BEFORE.replace(
            "world w { import host; export guest; }",
            "world w { import host; export guest; } world other { export guest; }",
        ))
        .unwrap_err();
        assert!(error.to_string().contains("one world"), "{error}");
    }
}
