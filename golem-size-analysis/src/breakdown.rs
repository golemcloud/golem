use anyhow::{Context, bail, ensure};
use serde::Serialize;
use std::collections::BTreeMap;

#[cfg(test)]
use crate::attribution::core_custom_section_bytes;
use crate::attribution::{Symbol, component_symbols, extra_custom_section_bytes, scale_to};
use crate::sections::section_table;

/// A published bucket of the Rule 2 breakdown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Bucket {
    /// The language runtime: the prebuilt JS guest, or the standard library of a compiled SDK.
    Runtime,
    /// JS runtime built-ins that are initialized only on first use.
    LazyBuiltins,
    Sdk,
    Dependencies,
    UserCode,
    /// The Scala.js bundle, which has no finer attribution.
    Bundle,
    /// The data that pre-initialization (Wizer) adds to the JS component.
    Snapshot,
    /// Bytes no symbol owns: the component wrapper, data segments, names and other metadata.
    Other,
}

/// Why a bucket's bytes are kept (spec 8.3, Rule 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Reason {
    Reachable,
    PossiblyReachable,
    RetainedForColdStart,
}

/// The reason for each bucket. A JS runtime can run code written after the build, so all of it
/// is reachable (Rule 3), and so is the bundle the build injected; lazily initialized built-ins
/// are possibly reachable; the snapshot is retained for cold start (Rule 8). Compiled SDKs keep
/// only what is reachable at build time (Rule 3), so their buckets are all reachable.
pub fn reason(bucket: Bucket) -> Reason {
    match bucket {
        Bucket::LazyBuiltins => Reason::PossiblyReachable,
        Bucket::Snapshot => Reason::RetainedForColdStart,
        Bucket::Runtime
        | Bucket::Sdk
        | Bucket::Dependencies
        | Bucket::UserCode
        | Bucket::Bundle
        | Bucket::Other => Reason::Reachable,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BucketRow {
    pub bucket: Bucket,
    pub bytes: u64,
    pub reason: Reason,
}

/// The published breakdown of one uploaded file. The rows add up to `bytes`. `approximate`
/// marks a breakdown measured on another build and scaled to this file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Breakdown {
    pub bytes: u64,
    pub approximate: bool,
    pub buckets: Vec<BucketRow>,
}

impl Breakdown {
    fn new(bytes: u64, approximate: bool, buckets: BTreeMap<Bucket, u64>) -> anyhow::Result<Self> {
        let sum: u64 = buckets.values().sum();
        ensure!(
            sum == bytes,
            "the buckets add up to {sum} bytes, not to the file's {bytes} bytes"
        );
        Ok(Self {
            bytes,
            approximate,
            buckets: buckets
                .into_iter()
                .map(|(bucket, bytes)| BucketRow {
                    bucket,
                    bytes,
                    reason: reason(bucket),
                })
                .collect(),
        })
    }

    pub fn bucket(&self, bucket: Bucket) -> u64 {
        self.buckets
            .iter()
            .find(|row| row.bucket == bucket)
            .map_or(0, |row| row.bytes)
    }
}

fn data_bytes(file: &[u8]) -> anyhow::Result<u64> {
    Ok(section_table(file)?
        .iter()
        .filter(|row| row.section == "data")
        .map(|row| row.bytes)
        .sum())
}

/// The Wizer snapshot of a JS component: the data-section growth of the preinitialized
/// component over its `.dynamic.wasm` input. Other sections, such as a stripped `name` section,
/// do not count, so this is not the size difference of an `optimized` and a `quick` build.
pub fn js_snapshot_bytes(dynamic: &[u8], preinitialized: &[u8]) -> anyhow::Result<u64> {
    let (before, after) = (data_bytes(dynamic)?, data_bytes(preinitialized)?);
    after.checked_sub(before).with_context(|| {
        format!("the preinitialized component has less data ({after}) than its input ({before})")
    })
}

/// The inputs of a JS component's breakdown.
pub struct JsComponent<'a> {
    /// The prebuilt guest the bundle was injected into (`agent_guest.wasm`).
    pub guest: &'a [u8],
    /// The guest with the bundle injected (`.dynamic.wasm`).
    pub dynamic: &'a [u8],
    /// The uploaded file: the preinitialized component, or `dynamic` itself when the preset does
    /// not pre-initialize.
    pub uploaded: &'a [u8],
}

impl JsComponent<'_> {
    fn bundle_and_snapshot(&self) -> anyhow::Result<(u64, u64)> {
        let bundle = (self.dynamic.len() as u64)
            .checked_sub(self.guest.len() as u64)
            .context("the injected component is smaller than its guest")?;
        let snapshot = if self.uploaded == self.dynamic {
            0
        } else {
            js_snapshot_bytes(self.dynamic, self.uploaded)?
        };
        Ok((bundle, snapshot))
    }

    fn runtime(&self, bundle: u64, snapshot: u64) -> anyhow::Result<u64> {
        (self.uploaded.len() as u64)
            .checked_sub(bundle + snapshot)
            .context("the bundle and snapshot are larger than the uploaded file")
    }
}

/// TypeScript and Effect: the guest is the runtime; the bytes the injection added are split into
/// SDK, dependencies and user code in proportion to the rollup module map (module id to rendered
/// length); the snapshot is the data growth from pre-initialization. The uploaded file's other
/// bytes, such as a stripped `name` section, are counted against the runtime.
pub fn js_breakdown(
    component: &JsComponent<'_>,
    module_map: &BTreeMap<String, u64>,
) -> anyhow::Result<Breakdown> {
    let (bundle, snapshot) = component.bundle_and_snapshot()?;
    let mut rendered: BTreeMap<Bucket, u64> = BTreeMap::new();
    for (id, bytes) in module_map {
        *rendered.entry(js_module_bucket(id)).or_default() += bytes;
    }
    if rendered.values().sum::<u64>() == 0 && bundle > 0 {
        bail!("the module map is empty, but the injection added {bundle} bytes");
    }
    let mut buckets = scale_to(&rendered, bundle);
    buckets.insert(Bucket::Snapshot, snapshot);
    buckets.insert(Bucket::Runtime, component.runtime(bundle, snapshot)?);
    Breakdown::new(component.uploaded.len() as u64, false, buckets)
}

/// In-tree Golem SDK sources, as rollup resolves them when an application links the SDKs from
/// the repository: the TS packages, the Effect SDK's sources and its HTTP contract package.
const IN_TREE_SDK_PATHS: [&str; 3] = [
    "sdks/ts/packages/",
    "sdks/effect/src/",
    "sdks/http-contract/",
];

/// The bucket of one rollup module id: the Golem SDK packages, other `node_modules` packages, or
/// the application's own modules (including virtual modules rollup generates for it).
pub fn js_module_bucket(id: &str) -> Bucket {
    let id = id.replace('\\', "/");
    match id.rfind("node_modules/") {
        Some(at) if id[at + "node_modules/".len()..].starts_with("@golemcloud/") => Bucket::Sdk,
        Some(_) => Bucket::Dependencies,
        None if IN_TREE_SDK_PATHS.iter().any(|path| id.contains(path)) => Bucket::Sdk,
        None => Bucket::UserCode,
    }
}

/// Scala: the guest is the runtime, the injected Scala.js bundle is one bucket, plus the
/// snapshot.
pub fn scala_breakdown(component: &JsComponent<'_>) -> anyhow::Result<Breakdown> {
    let (bundle, snapshot) = component.bundle_and_snapshot()?;
    let buckets = BTreeMap::from([
        (Bucket::Bundle, bundle),
        (Bucket::Snapshot, snapshot),
        (Bucket::Runtime, component.runtime(bundle, snapshot)?),
    ]);
    Breakdown::new(component.uploaded.len() as u64, false, buckets)
}

/// How a compiled SDK names the owner of a symbol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolNaming {
    /// Demangled Rust paths: the crate is the first path segment.
    Rust,
    /// Go `path/package.Member` names: the owner is the package path.
    Go,
    /// MoonBit `$module/package.Member` names: the owner is the package path.
    Moonbit,
}

/// Assigns the symbols of a compiled SDK to buckets by their owner: the crate (Rust) or the
/// package path (Go, MoonBit). Owners in `user_code`, `sdk` and `runtime` (a name, or a prefix
/// ending in `/`) go to those buckets; Go standard-library packages go to the runtime; every
/// other owner is a dependency. Symbols with no owner, such as imports, data segments and
/// section headers, go to [`Bucket::Other`].
#[derive(Debug, Clone)]
pub struct SymbolClassifier {
    pub naming: SymbolNaming,
    pub user_code: Vec<String>,
    pub sdk: Vec<String>,
    pub runtime: Vec<String>,
}

impl SymbolClassifier {
    /// Rust: crate names. `user_crate` is the component crate's name with `-` replaced by `_`.
    pub fn rust(user_crate: &str) -> Self {
        let names = |names: &[&str]| names.iter().map(|n| n.to_string()).collect();
        Self {
            naming: SymbolNaming::Rust,
            user_code: vec![user_crate.to_string()],
            sdk: names(&[
                "golem_rust",
                "golem_rust_macro",
                "golem_schema",
                "golem_wasm",
                "wit_bindgen",
                "wit_bindgen_rt",
            ]),
            runtime: names(&[
                "core",
                "alloc",
                "std",
                "compiler_builtins",
                "dlmalloc",
                "wasi",
                "panic_abort",
                "panic_unwind",
            ]),
        }
    }

    /// Go: package paths. `user_module` is the application's module path.
    pub fn go(user_module: &str) -> Self {
        Self {
            naming: SymbolNaming::Go,
            user_code: vec![
                user_module.to_string(),
                format!("{user_module}/"),
                "main".to_string(),
            ],
            sdk: vec!["github.com/golemcloud/golem/sdks/go/".to_string()],
            runtime: Vec::new(),
        }
    }

    /// MoonBit: package paths. `user_module` is the application's module name.
    pub fn moonbit(user_module: &str) -> Self {
        Self {
            naming: SymbolNaming::Moonbit,
            user_code: vec![user_module.to_string(), format!("{user_module}/")],
            sdk: vec![
                "golemcloud/golem_sdk".to_string(),
                "golemcloud/golem_sdk/".to_string(),
            ],
            runtime: vec![
                "moonbitlang/core".to_string(),
                "moonbitlang/core/".to_string(),
            ],
        }
    }

    fn classify(&self, name: &str) -> Bucket {
        let Some(owner) = owner(self.naming, name) else {
            return Bucket::Other;
        };
        let owner = owner.as_ref();
        let matches = |owners: &[String]| {
            owners.iter().any(|o| {
                if o.ends_with('/') {
                    owner.starts_with(o.as_str())
                } else {
                    owner == o
                }
            })
        };
        if matches(&self.user_code) {
            Bucket::UserCode
        } else if matches(&self.sdk) {
            Bucket::Sdk
        } else if matches(&self.runtime)
            || (self.naming == SymbolNaming::Go && is_go_runtime(owner))
        {
            Bucket::Runtime
        } else {
            Bucket::Dependencies
        }
    }
}

/// The owner of a twiggy item: the crate of a Rust symbol, or the package path of a Go or
/// MoonBit symbol. `None` for imports, data segments, section headers and other items no crate
/// or package owns.
pub fn owner(naming: SymbolNaming, name: &str) -> Option<std::borrow::Cow<'_, str>> {
    match naming {
        SymbolNaming::Rust => rust_crate(name).map(Into::into),
        SymbolNaming::Go => package_path(name).map(Into::into),
        SymbolNaming::Moonbit => moonbit_package(name),
    }
}

/// Whether a twiggy item names something other than a function or global: an import, a data
/// segment, a section header or another structural item.
fn is_structural(name: &str) -> bool {
    name.starts_with("import ")
        || name.starts_with("export ")
        || name.starts_with('"')
        || name.contains(" section")
        || name.starts_with("wasm magic")
        || name.starts_with('<') && name.ends_with('>') && !name.contains("::")
}

/// The first path segment of a Rust path, without a v0 disambiguator and leading `&`, `*`,
/// `mut` or `dyn`.
fn first_segment(path: &str) -> Option<&str> {
    let path = path
        .trim_start_matches(['&', '*'])
        .trim_start_matches("mut ");
    let path = path.strip_prefix("dyn ").unwrap_or(path);
    let (head, _) = path.split_once("::")?;
    let head = head.split_once('[').map_or(head, |(ident, _)| ident);
    is_rust_ident(head).then_some(head)
}

fn is_rust_ident(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_alphanumeric() || c == '_')
}

/// The crate of a demangled Rust symbol: the first path segment without its v0 disambiguator.
/// For a trait impl `<T as Trait>::f` it is the crate of `T`, or of `Trait` when `T` is not a
/// path (a slice, tuple, reference, `dyn` type or primitive).
fn rust_crate(name: &str) -> Option<&str> {
    if is_structural(name) {
        return None;
    }
    match name.strip_prefix('<') {
        Some(impl_) => {
            let (self_ty, trait_) = split_impl(impl_)?;
            if self_ty.contains("::")
                && !self_ty.starts_with(['[', '('])
                && !self_ty.starts_with("dyn ")
            {
                first_segment(self_ty).or_else(|| first_segment(trait_?))
            } else {
                trait_
                    .and_then(first_segment)
                    .or_else(|| first_segment(self_ty))
            }
        }
        None => first_segment(name),
    }
}

/// Splits `T as Trait>::rest` into `T` and `Trait`, respecting nested angle brackets.
fn split_impl(impl_: &str) -> Option<(&str, Option<&str>)> {
    let mut depth = 0usize;
    let mut as_at = None;
    for (i, c) in impl_.char_indices() {
        match c {
            '<' => depth += 1,
            '>' if depth == 0 => {
                let inner = &impl_[..i];
                return Some(match as_at {
                    Some(at) => (&inner[..at], Some(&inner[at + 4..])),
                    None => (inner, None),
                });
            }
            '>' => depth -= 1,
            ' ' if depth == 0 && impl_[i..].starts_with(" as ") && as_at.is_none() => {
                as_at = Some(i)
            }
            _ => {}
        }
    }
    None
}

/// The package path of a Go or MoonBit symbol: everything before the first `.` after the last
/// `/` (generic arguments in brackets do not count), without a leading `$` or `@`.
fn package_path(name: &str) -> Option<&str> {
    if is_structural(name) {
        return None;
    }
    let name = name.trim_start_matches(['$', '@']);
    let head = name.split(['[', '(', ' ']).next().unwrap_or(name);
    let last_slash = head.rfind('/').map_or(0, |i| i + 1);
    let dot = head[last_slash..].find('.')? + last_slash;
    let package = &name[..dot];
    (!package.is_empty()).then_some(package)
}

/// The package of a mangled MoonBit symbol. `moonc` writes `_M0` followed by a kind letter (`F`
/// function, `M` method, `I` impl, `T` type and others) and the package: `PB` for
/// `moonbitlang/core/builtin`, `PC<count><segments>` for another `moonbitlang/core` package,
/// `P0` for code the compiler generates, and `P<count><segments>` for any other package. Each
/// segment is its length and its text, with `_` doubled. The count and the first length are
/// adjacent digits, so the count is the split for which every segment fits in the name.
/// Runtime helpers that the MoonBit compiler links in (`moonbit.*`, `tlsf/*`) belong to
/// `moonbitlang/core`.
fn moonbit_package(name: &str) -> Option<std::borrow::Cow<'_, str>> {
    if name.starts_with("moonbit.") || name.starts_with("tlsf/") {
        return Some("moonbitlang/core".into());
    }
    let rest = name.strip_prefix("_M0")?;
    let rest = rest.get(1..)?.strip_prefix('P')?;
    if rest.starts_with('B') {
        return Some("moonbitlang/core/builtin".into());
    }
    if rest.starts_with('0') {
        return None;
    }
    let (prefix, rest) = match rest.strip_prefix('C') {
        Some(core) => ("moonbitlang/core/", core),
        None => ("", rest),
    };
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    (1..=digits.min(3)).find_map(|split| {
        let count: usize = rest[..split].parse().ok()?;
        let segments = moonbit_segments(&rest[split..], count)?;
        Some(format!("{prefix}{}", segments.join("/")).into())
    })
}

/// Reads `count` length-prefixed segments, undoubling `_`; `None` when they do not fit.
fn moonbit_segments(mut rest: &str, count: usize) -> Option<Vec<String>> {
    let mut segments = Vec::with_capacity(count);
    for _ in 0..count {
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 || rest.starts_with('0') {
            return None;
        }
        let length: usize = rest[..digits].parse().ok()?;
        let segment = rest.get(digits..digits + length)?;
        if segment.is_empty()
            || !segment
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        {
            return None;
        }
        segments.push(segment.replace("__", "_"));
        rest = &rest[digits + length..];
    }
    // The next item (a name, a type or a trait) starts with a length or a package marker.
    rest.starts_with(|c: char| c.is_ascii_digit() || c == 'P')
        .then_some(segments)
}

/// The top-level directories of `src/` in the Go toolchain the CLI pins (`go1.27.1-golem.5`, the
/// same set as upstream `go1.27.1`). Every standard-library package path starts with one of them.
/// A local module with a dotless path, such as `module shared`, is not in this list.
const GO_STANDARD_LIBRARY: &[&str] = &[
    "archive",
    "arena",
    "bufio",
    "builtin",
    "bytes",
    "cmd",
    "cmp",
    "compress",
    "container",
    "context",
    "crypto",
    "database",
    "debug",
    "embed",
    "encoding",
    "errors",
    "expvar",
    "flag",
    "fmt",
    "go",
    "hash",
    "html",
    "image",
    "index",
    "internal",
    "io",
    "iter",
    "log",
    "maps",
    "math",
    "mime",
    "net",
    "os",
    "path",
    "plugin",
    "reflect",
    "regexp",
    "runtime",
    "simd",
    "slices",
    "sort",
    "strconv",
    "strings",
    "structs",
    "sync",
    "syscall",
    "testing",
    "text",
    "time",
    "unicode",
    "unique",
    "unsafe",
    "uuid",
    "weak",
];

/// Go packages compiled from the toolchain: the standard library, its vendored packages, and
/// linker-generated `type:` and `go:` symbols.
fn is_go_runtime(package: &str) -> bool {
    package.starts_with("vendor/")
        || package.starts_with("type:")
        || package.starts_with("go:")
        || GO_STANDARD_LIBRARY.contains(&package.split('/').next().unwrap_or(package))
}

/// Buckets of the named symbols of every core module of `file`. Bytes outside the core
/// modules (the component wrapper) are [`Bucket::Other`], so the buckets add up to the file.
pub fn symbol_buckets(
    file: &[u8],
    classifier: &SymbolClassifier,
) -> anyhow::Result<(BTreeMap<Bucket, u64>, Vec<Symbol>)> {
    let mut buckets: BTreeMap<Bucket, u64> = BTreeMap::new();
    let mut all = Vec::new();
    let mut in_cores = 0u64;
    for (core, symbols) in component_symbols(file)? {
        in_cores += core.len() as u64;
        match symbols {
            Some(symbols) => {
                for symbol in &symbols {
                    *buckets
                        .entry(classifier.classify(&symbol.name))
                        .or_default() += symbol.bytes;
                }
                all.extend(symbols);
            }
            None => *buckets.entry(Bucket::Other).or_default() += core.len() as u64,
        }
    }
    *buckets.entry(Bucket::Other).or_default() += file.len() as u64 - in_cores;
    Ok((buckets, all))
}

/// Rust: twiggy crate attribution of a named build of the uploaded file.
pub fn rust_breakdown(
    file: &[u8],
    classifier: &SymbolClassifier,
) -> anyhow::Result<(Breakdown, Vec<Symbol>)> {
    let (buckets, symbols) = symbol_buckets(file, classifier)?;
    Ok((Breakdown::new(file.len() as u64, false, buckets)?, symbols))
}

/// Attributes a named build and scales its buckets to the uploaded file: Go and MoonBit release
/// builds, and a Rust build whose uploaded file is stripped. The custom sections that only the
/// named build carries (its `name` section and DWARF `.debug_*` sections) have no counterpart in
/// the uploaded file, so they are left out before scaling instead of inflating `other` and
/// shrinking every attributed bucket. The result is approximate.
pub fn scaled_breakdown(
    named: &[u8],
    uploaded: &[u8],
    classifier: &SymbolClassifier,
) -> anyhow::Result<(Breakdown, Vec<Symbol>)> {
    let (mut buckets, symbols) = symbol_buckets(named, classifier)?;
    let other = buckets.entry(Bucket::Other).or_default();
    *other = other
        .checked_sub(extra_custom_section_bytes(named, uploaded)?)
        .context(
            "the named build's extra custom sections are larger than its unattributed bytes",
        )?;
    let buckets = scale_to(&buckets, uploaded.len() as u64);
    Ok((
        Breakdown::new(uploaded.len() as u64, true, buckets)?,
        symbols,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sections::COMPONENT_CORE_MODULE_SECTION;
    use crate::sections::tests::{component, section};
    use test_r::test;

    fn wrap(cores: &[Vec<u8>]) -> Vec<u8> {
        component(
            &cores
                .iter()
                .map(|core| section(COMPONENT_CORE_MODULE_SECTION, core))
                .collect::<Vec<_>>(),
        )
    }

    /// A guest core: runtime code plus `data` bytes of data, with function names when `named`.
    fn js_core(data: usize, extra_name: &str) -> Vec<u8> {
        wat::parse_str(format!(
            r#"(module (memory 1)
                (data (i32.const 0) "{}")
                (func ${extra_name} (export "run") (result i32) i32.const 1))"#,
            "x".repeat(data)
        ))
        .unwrap()
    }

    #[test]
    fn snapshot_is_the_data_growth_only_not_the_size_difference() {
        let dynamic = wrap(&[js_core(100, "a_long_function_name_that_preinit_strips")]);
        let preinitialized = wrap(&[js_core(350, "f")]);
        // Data sections with framing: 1 + 2 + 1 + (1 + 3 + 2 + 350) against 1 + 1 + 1 + (1 + 3 + 1 + 100).
        assert_eq!(
            js_snapshot_bytes(&dynamic, &preinitialized).unwrap(),
            360 - 108
        );
        // The size difference also subtracts the shorter `name` section.
        assert_ne!(preinitialized.len() - dynamic.len(), 252);
        assert!(js_snapshot_bytes(&preinitialized, &dynamic).is_err());
    }

    #[test]
    fn reasons_follow_rules_2_3_and_8() {
        assert_eq!(reason(Bucket::Runtime), Reason::Reachable);
        assert_eq!(reason(Bucket::Bundle), Reason::Reachable);
        assert_eq!(reason(Bucket::LazyBuiltins), Reason::PossiblyReachable);
        assert_eq!(reason(Bucket::Snapshot), Reason::RetainedForColdStart);
        assert_eq!(
            serde_json::to_value(reason(Bucket::Snapshot)).unwrap(),
            "retained-for-cold-start"
        );
    }

    #[test]
    fn js_bundle_is_split_by_module_map_and_kept_out_of_the_runtime() {
        let guest = wrap(&[js_core(1000, "runtime")]);
        let dynamic = wrap(&[js_core(1600, "runtime")]);
        let uploaded = wrap(&[js_core(2000, "r")]);
        let module_map = BTreeMap::from([
            ("src/main.ts".to_string(), 300),
            (
                "node_modules/@golemcloud/golem-ts-sdk/dist/index.mjs".to_string(),
                200,
            ),
            (
                "node_modules/.pnpm/zod@3/node_modules/zod/lib/index.mjs".to_string(),
                100,
            ),
        ]);
        let component = JsComponent {
            guest: &guest,
            dynamic: &dynamic,
            uploaded: &uploaded,
        };
        let breakdown = js_breakdown(&component, &module_map).unwrap();

        let bundle = (dynamic.len() - guest.len()) as u64;
        assert_eq!(breakdown.bucket(Bucket::Snapshot), 400);
        assert_eq!(breakdown.bucket(Bucket::UserCode), bundle / 2);
        assert_eq!(breakdown.bucket(Bucket::Sdk), bundle / 3);
        assert_eq!(breakdown.bucket(Bucket::Dependencies), bundle / 6);
        assert_eq!(
            breakdown.bucket(Bucket::Runtime),
            uploaded.len() as u64 - bundle - 400
        );
        assert_eq!(
            breakdown.buckets.iter().map(|r| r.bytes).sum::<u64>(),
            uploaded.len() as u64
        );
        assert!(!breakdown.approximate);

        let quick = JsComponent {
            guest: &guest,
            dynamic: &dynamic,
            uploaded: &dynamic,
        };
        assert_eq!(
            js_breakdown(&quick, &module_map)
                .unwrap()
                .bucket(Bucket::Snapshot),
            0
        );
        assert_eq!(
            scala_breakdown(&component).unwrap().bucket(Bucket::Bundle),
            bundle
        );
    }

    #[test]
    fn js_module_ids_map_to_sdk_dependency_and_user_buckets() {
        assert_eq!(
            js_module_bucket("/app/node_modules/@golemcloud/effect-golem/x.js"),
            Bucket::Sdk
        );
        assert_eq!(
            js_module_bucket("C:\\app\\node_modules\\effect\\x.js"),
            Bucket::Dependencies
        );
        assert_eq!(
            js_module_bucket("../../sdks/ts/packages/golem-ts-sdk/dist/x.mjs"),
            Bucket::Sdk
        );
        assert_eq!(js_module_bucket("\0virtual:agent-main"), Bucket::UserCode);
        assert_eq!(js_module_bucket("src/agents/counter.ts"), Bucket::UserCode);
        // Linked in-tree SDK packages resolve to their real paths.
        assert_eq!(
            js_module_bucket("/repo/sdks/http-contract/dist/index.js"),
            Bucket::Sdk
        );
        assert_eq!(
            js_module_bucket("/repo/sdks/ts/packages/golem-ts-bridge/dist/index.mjs"),
            Bucket::Sdk
        );
        assert_eq!(
            js_module_bucket("/repo/sdks/effect/src/Agent.ts"),
            Bucket::Sdk
        );
        // An application inside an SDK directory is user code.
        assert_eq!(
            js_module_bucket("/repo/sdks/effect/integration-test/src/main.ts"),
            Bucket::UserCode
        );
    }

    fn rust_core(names: bool) -> Vec<u8> {
        let name = |n: &str| {
            if names {
                format!("${n}")
            } else {
                String::new()
            }
        };
        wat::parse_str(format!(
            r#"(module (memory 1) (data (i32.const 0) "0123456789")
                (func {} (export "a") (result i32)
                    i32.const 1 i32.const 2 i32.add i32.const 3 i32.mul i32.const 4 i32.sub)
                (func {} (export "b") (result i32) i32.const 7 i32.const 8 i32.add)
                (func {} (export "c") (result i32) i32.const 9)
                (func {} (export "d") (result i32) i32.const 5))"#,
            name("_ZN10my_counter5Agent3inc17h0123456789abcdefE"),
            name("_ZN10golem_rust5agent6invoke17h0123456789abcdefE"),
            name("_ZN4core3fmt5write17h0123456789abcdefE"),
            name("_ZN5regex4exec17h0123456789abcdefE"),
        ))
        .unwrap()
    }

    #[test]
    fn rust_symbols_land_in_their_crate_buckets_and_add_up() {
        let file = wrap(&[
            rust_core(true),
            wat::parse_str("(module (memory 1))").unwrap(),
        ]);
        let (breakdown, symbols) =
            rust_breakdown(&file, &SymbolClassifier::rust("my_counter")).unwrap();
        assert!(breakdown.bucket(Bucket::UserCode) > breakdown.bucket(Bucket::Sdk));
        assert!(breakdown.bucket(Bucket::Sdk) > breakdown.bucket(Bucket::Runtime));
        assert!(breakdown.bucket(Bucket::Dependencies) > 0);
        assert!(breakdown.bucket(Bucket::Other) > 0);
        assert_eq!(
            breakdown.buckets.iter().map(|r| r.bytes).sum::<u64>(),
            file.len() as u64
        );
        assert!(symbols.iter().any(|s| s.name.starts_with("regex::exec")));

        let stripped = wrap(&[rust_core(false)]);
        let error = rust_breakdown(&stripped, &SymbolClassifier::rust("my_counter"))
            .unwrap_err()
            .to_string();
        assert!(error.contains("no function names"), "{error}");
    }

    #[test]
    fn rust_symbols_are_bucketed_by_owning_crate() {
        let rust = SymbolClassifier::rust("my_counter");
        for (name, bucket) in [
            ("my_counter::Agent::inc::h0123", Bucket::UserCode),
            ("golem_rust[abc012]::agent::invoke", Bucket::Sdk),
            (
                "<my_counter::Agent as golem_rust::Agent>::invoke",
                Bucket::UserCode,
            ),
            ("<golem_rust::Type as core::fmt::Debug>::fmt", Bucket::Sdk),
            // Trait impls on types that are not paths belong to the trait's crate.
            ("<[u8] as core::fmt::Debug>::fmt", Bucket::Runtime),
            ("<&mut W as core::fmt::Write>::write_str", Bucket::Runtime),
            (
                "<dyn core::any::Any as core::fmt::Debug>::fmt",
                Bucket::Runtime,
            ),
            ("<() as golem_rust::Schema>::schema", Bucket::Sdk),
            (
                "<(u32, regex::Regex) as serde::Serialize>::serialize",
                Bucket::Dependencies,
            ),
            (
                "<alloc::vec::Vec<T> as regex::Pattern>::find",
                Bucket::Runtime,
            ),
            ("regex::exec::h0123", Bucket::Dependencies),
            // Imports and structural items have no crate.
            (
                "import wasi:cli/environment@0.2.6::get-environment",
                Bucket::Other,
            ),
            ("import golem_rust::host::get-agent-type", Bucket::Other),
            ("data[0]", Bucket::Other),
            ("code section headers", Bucket::Other),
            ("\"function names\" subsection", Bucket::Other),
            ("<meta root>", Bucket::Other),
        ] {
            assert_eq!(rust.classify(name), bucket, "{name}");
        }
    }

    #[test]
    fn go_and_moonbit_symbols_are_bucketed_by_package_path() {
        let go = SymbolClassifier::go("example.com/app");
        for (name, bucket) in [
            ("example.com/app.Counter.Inc", Bucket::UserCode),
            ("example.com/app/internal/store.Get", Bucket::UserCode),
            ("main.main", Bucket::UserCode),
            (
                "github.com/golemcloud/golem/sdks/go/golem.Invoke",
                Bucket::Sdk,
            ),
            ("github.com/google/uuid.New", Bucket::Dependencies),
            ("runtime.mallocgc", Bucket::Runtime),
            ("unique.Make[go.shape.string]", Bucket::Runtime),
            ("compress/flate.NewWriter", Bucket::Runtime),
            (
                "vendor/golang.org/x/net/http2/hpack.Encode",
                Bucket::Runtime,
            ),
            ("type:.eq.[2]interface {}", Bucket::Runtime),
            ("go:itab.*os.File,io.Writer", Bucket::Runtime),
            ("mime/multipart.NewReader", Bucket::Runtime),
            // A second local module with a dotless path is a dependency, not the runtime.
            ("shared/store.Get", Bucket::Dependencies),
            ("component-name/helpers.Format", Bucket::Dependencies),
            (
                "import wasi:io/streams@0.2.0::[method]output-stream.write",
                Bucket::Other,
            ),
        ] {
            assert_eq!(go.classify(name), bucket, "{name}");
        }
        // The Go standard-library rule does not apply to other SDKs.
        assert_eq!(
            SymbolClassifier::moonbit("me/app").classify("_M0FP26unique3pkg1f"),
            Bucket::Dependencies
        );
        assert_eq!(
            SymbolClassifier::rust("app").classify("fmt::write"),
            Bucket::Dependencies
        );
    }

    #[test]
    fn go_and_moonbit_debug_buckets_are_scaled_to_the_release_file() {
        let debug = wrap(&[wat::parse_str(
            r#"(module (memory 1)
                (func $github.com/golemcloud/golem/sdks/go/golem.Invoke (result i32)
                    i32.const 1 i32.const 2 i32.add i32.const 3 i32.add)
                (func $runtime.mallocgc (result i32) i32.const 1 i32.const 2 i32.add)
                (func $example.com/app.Counter.Inc (result i32) i32.const 1)
                (func $github.com/google/uuid.New (result i32) i32.const 2))"#,
        )
        .unwrap()]);
        // A stripped release build: no `name` section, padded to 777 bytes with a custom
        // section, so nothing the debug build has beyond it is shared.
        let stripped =
            wat::parse_str("(module (memory 1) (func (result i32) i32.const 1))").unwrap();
        let padding = 777 - stripped.len() - 1 - 2 - 1 - "padding".len();
        let release = [
            stripped,
            crate::sections::tests::custom("padding", &vec![0u8; padding]),
        ]
        .concat();
        assert_eq!(release.len(), 777);
        let classifier = SymbolClassifier::go("example.com/app");
        let (unscaled, _) = symbol_buckets(&debug, &classifier).unwrap();
        assert!(unscaled[&Bucket::Sdk] > 0 && unscaled[&Bucket::Runtime] > 0);
        assert!(unscaled[&Bucket::UserCode] > 0 && unscaled[&Bucket::Dependencies] > 0);

        let (breakdown, _) = scaled_breakdown(&debug, &release, &classifier).unwrap();
        assert!(breakdown.approximate);
        assert_eq!(breakdown.bytes, 777);
        assert_eq!(breakdown.buckets.iter().map(|r| r.bytes).sum::<u64>(), 777);
        assert!(breakdown.bucket(Bucket::Sdk) > breakdown.bucket(Bucket::UserCode));

        // The debug build's `name` section is left out before scaling: each attributed bucket
        // keeps its share of the debug build without that section.
        let names = extra_custom_section_bytes(&debug, &release).unwrap();
        assert!(names > 0);
        let attributable = debug.len() as u64 - names;
        let expected_sdk = unscaled[&Bucket::Sdk] * 777 / attributable;
        assert!(
            (expected_sdk..=expected_sdk + 1).contains(&breakdown.bucket(Bucket::Sdk)),
            "{breakdown:?}, expected about {expected_sdk}"
        );
    }

    #[test]
    fn dwarf_and_shared_custom_sections_are_handled_before_scaling() {
        use crate::sections::tests::custom;
        let core = |extra: &[Vec<u8>]| {
            let mut core = wat::parse_str(
                r#"(module (memory 1)
                    (func $github.com/golemcloud/golem/sdks/go/golem.Invoke (result i32)
                        i32.const 1 i32.const 2 i32.add i32.const 3 i32.add)
                    (func $runtime.mallocgc (result i32) i32.const 1 i32.const 2 i32.add)
                    (func $example.com/app.Counter.Inc (result i32) i32.const 1))"#,
            )
            .unwrap();
            core.extend(extra.concat());
            core
        };
        let producers = custom("producers", &[7u8; 50]);
        let release = wrap(&[core(std::slice::from_ref(&producers))]);
        let classifier = SymbolClassifier::go("example.com/app");
        let scaled = |extra: &[Vec<u8>]| {
            let named = wrap(&[core(extra)]);
            scaled_breakdown(&named, &release, &classifier).unwrap().0
        };
        let plain = scaled(std::slice::from_ref(&producers));
        let with_dwarf = scaled(&[
            producers.clone(),
            custom(".debug_info", &[0u8; 4000]),
            custom(".debug_line", &[0u8; 1500]),
        ]);
        assert_eq!(with_dwarf, plain);
        assert!(plain.bucket(Bucket::Sdk) > 0 && plain.bucket(Bucket::UserCode) > 0);
        // Only what the named build has beyond the uploaded file is left out: here its `name`
        // section, because the uploaded file is stripped, and not the shared `producers`.
        let named = wrap(&[core(std::slice::from_ref(&producers))]);
        let release_stripped = wrap(&[[
            wat::parse_str("(module (memory 1) (func (result i32) i32.const 1))").unwrap(),
            producers,
        ]
        .concat()]);
        let named_sections = core_custom_section_bytes(&named).unwrap();
        assert_eq!(
            extra_custom_section_bytes(&named, &release_stripped).unwrap(),
            named_sections["name"]
        );
    }

    #[test]
    fn moonbit_mangled_names_are_bucketed_by_package() {
        // Names from `moon build --target wasm --debug` (moonc v0.10.18) of a module
        // `username/hello` that uses the Golem SDK and an external package.
        let moonbit = SymbolClassifier::moonbit("username/hello");
        for (name, bucket) in [
            ("_M0FP28username5hello12use__generic", Bucket::UserCode),
            (
                "_M0FP48username5hello4deep5inner8deep__fn",
                Bucket::UserCode,
            ),
            (
                "_M0FP138username5hello1a1b1c1d1e1f1g1h1i1j1k3kfn",
                Bucket::UserCode,
            ),
            (
                "_M0FP38username5hello52long__package__name__that__is__quite__long__abcdefgh3lfn",
                Bucket::UserCode,
            ),
            ("_M0MPB3Map3setGsRP28username5hello4ItemE", Bucket::Runtime),
            (
                "_M0MPC15array5Array4pushGRP28username5hello4ItemE",
                Bucket::Runtime,
            ),
            ("_M0IPC16string6StringPB4Hash4hash", Bucket::Runtime),
            ("_M0FPB20int__to__string__dec", Bucket::Runtime),
            ("moonbit.utf8.encode_from_utf16", Bucket::Runtime),
            ("tlsf/insertBlock", Bucket::Runtime),
            (
                "_M0FP310golemcloud10golem__sdk6agents14register__agent",
                Bucket::Sdk,
            ),
            ("_M0FP26mizchi5bench9ackermann", Bucket::Dependencies),
            // Code the compiler generates, and structural items, have no package.
            ("_M0FP017____moonbit__main", Bucket::Other),
            (
                "_M0IP016_24default__implPB6Logger5writeGRPB13StringBuilderE",
                Bucket::Other,
            ),
            ("import wasi_snapshot_preview1::fd_write", Bucket::Other),
            ("\"function names\" subsection", Bucket::Other),
        ] {
            assert_eq!(moonbit.classify(name), bucket, "{name}");
        }
        assert_eq!(
            owner(
                SymbolNaming::Moonbit,
                "_M0FP138username5hello1a1b1c1d1e1f1g1h1i1j1k3kfn"
            )
            .as_deref(),
            Some("username/hello/a/b/c/d/e/f/g/h/i/j/k")
        );
    }

    #[test]
    fn the_same_input_gives_byte_identical_json() {
        let file = wrap(&[rust_core(true)]);
        let classifier = SymbolClassifier::rust("my_counter");
        let render = || serde_json::to_vec(&rust_breakdown(&file, &classifier).unwrap()).unwrap();
        assert_eq!(render(), render());
    }
}
