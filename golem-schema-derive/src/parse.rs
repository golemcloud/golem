// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Attribute parsing for the schema derive macros.
//!
//! The full attribute surface is a single `#[schema(...)]` namespace. The
//! parsed forms below are deliberately permissive — anything that fails to
//! make sense in a given context surfaces as a `compile_error!` from
//! [`crate::expand`].

use darling::ast::NestedMeta;
use darling::{FromMeta, util::SpannedValue};
use syn::spanned::Spanned;
use syn::{Attribute, Lit, Meta, Path};

const SCHEMA_ATTR: &str = "schema";

/// Top-level `#[schema(...)]` flags on a type definition.
#[derive(Default, Debug, Clone)]
pub struct TypeAttrs {
    pub named: Option<String>,
    pub doc: Option<String>,
    pub alias: Vec<String>,
    pub example: Vec<String>,
    pub deprecated: Option<DeprecatedMarker>,
    pub role: Option<String>,
    pub union: bool,
    /// `#[schema(transparent)]` — newtype tuple structs with exactly one
    /// field delegate to the inner type instead of getting a graph slot.
    pub transparent: bool,
    /// `#[schema(rename_all = "...")]` — default rename strategy for fields
    /// and variant cases. Defaults to preserving the native Rust identifier so
    /// generated bridges show identifiers exactly as written in the user's
    /// code.
    pub rename_all: RenameAll,
    /// `#[schema(tag = "…", content = "…")]` — adjacently-tagged variant
    /// emission. Mutually exclusive with `#[schema(union)]`.
    pub adjacent_tag: Option<String>,
    pub adjacent_content: Option<String>,
}

/// Strategy for default field / case naming.
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenameAll {
    /// Keep the native Rust identifier verbatim.
    #[default]
    Preserve,
    Kebab,
    Snake,
    Camel,
    Pascal,
    ScreamingSnake,
}

impl RenameAll {
    fn from_string(raw: &str, span: proc_macro2::Span) -> syn::Result<Self> {
        match raw {
            "preserve" => Ok(Self::Preserve),
            "kebab-case" => Ok(Self::Kebab),
            "snake_case" => Ok(Self::Snake),
            "camelCase" => Ok(Self::Camel),
            "PascalCase" => Ok(Self::Pascal),
            "SCREAMING_SNAKE_CASE" => Ok(Self::ScreamingSnake),
            other => Err(syn::Error::new(
                span,
                format!(
                    "unknown rename_all strategy `{other}` (expected one of `preserve`, `kebab-case`, `snake_case`, `camelCase`, `PascalCase`, `SCREAMING_SNAKE_CASE`)"
                ),
            )),
        }
    }
}

#[derive(Debug, Clone)]
pub enum DeprecatedMarker {
    Flag,
    Message(String),
}

impl DeprecatedMarker {
    pub fn message(&self) -> String {
        match self {
            DeprecatedMarker::Flag => "deprecated".to_string(),
            DeprecatedMarker::Message(msg) => msg.clone(),
        }
    }
}

/// `#[schema(...)]` on a struct field or variant case.
#[derive(Default, Debug, Clone)]
pub struct ItemAttrs {
    pub rename: Option<String>,
    pub doc: Option<String>,
    pub alias: Vec<String>,
    pub example: Vec<String>,
    pub deprecated: Option<DeprecatedMarker>,
    pub source: Option<String>,
    pub source_kind: Option<String>,

    pub rich: Option<RichSpec>,

    pub discriminator: Option<DiscriminatorAttr>,

    /// `#[schema(skip)]` — omit field from the schema and value; populate
    /// from `Default::default()` on decode.
    pub skip: bool,
    /// `#[schema(default = "path::to::fn")]` — like `skip`, but uses the
    /// named function to produce the default value.
    pub default_with: Option<String>,
    /// `#[schema(flatten)]` — inline a record field's fields into the parent
    /// record (only valid on a field whose schema is a record).
    pub flatten: bool,
}

#[derive(Debug, Clone)]
pub enum RichSpec {
    Text(TextSpec),
    Binary(BinarySpec),
    Path(PathAttrSpec),
    Url(UrlSpec),
    Quantity(QuantitySpec),
    QuotaToken(QuotaTokenAttrSpec),
}

#[derive(Default, Debug, Clone, FromMeta)]
pub struct TextSpecRaw {
    #[darling(default)]
    pub language: Option<String>,
    #[darling(default)]
    pub languages: Option<StringList>,
    #[darling(default)]
    pub min: Option<u32>,
    #[darling(default)]
    pub max: Option<u32>,
    #[darling(default)]
    pub regex: Option<String>,
}

#[derive(Default, Debug, Clone)]
pub struct TextSpec {
    pub language: Option<String>,
    pub languages: Option<Vec<String>>,
    pub min: Option<u32>,
    pub max: Option<u32>,
    pub regex: Option<String>,
}

impl From<TextSpecRaw> for TextSpec {
    fn from(value: TextSpecRaw) -> Self {
        Self {
            language: value.language,
            languages: value.languages.map(|s| s.0),
            min: value.min,
            max: value.max,
            regex: value.regex,
        }
    }
}

#[derive(Default, Debug, Clone, FromMeta)]
pub struct BinarySpecRaw {
    #[darling(default)]
    pub mime_type: Option<String>,
    #[darling(default)]
    pub mime_types: Option<StringList>,
    #[darling(default)]
    pub min_bytes: Option<u32>,
    #[darling(default)]
    pub max_bytes: Option<u32>,
}

#[derive(Default, Debug, Clone)]
pub struct BinarySpec {
    pub mime_type: Option<String>,
    pub mime_types: Option<Vec<String>>,
    pub min_bytes: Option<u32>,
    pub max_bytes: Option<u32>,
}

impl From<BinarySpecRaw> for BinarySpec {
    fn from(value: BinarySpecRaw) -> Self {
        Self {
            mime_type: value.mime_type,
            mime_types: value.mime_types.map(|s| s.0),
            min_bytes: value.min_bytes,
            max_bytes: value.max_bytes,
        }
    }
}

#[derive(Debug, Clone, FromMeta)]
pub struct PathAttrSpecRaw {
    pub direction: SpannedValue<String>,
    pub kind: SpannedValue<String>,
    #[darling(default)]
    pub allowed_mime_types: Option<StringList>,
    #[darling(default)]
    pub allowed_extensions: Option<StringList>,
}

#[derive(Debug, Clone)]
pub struct PathAttrSpec {
    pub direction: SpannedValue<String>,
    pub kind: SpannedValue<String>,
    pub allowed_mime_types: Option<Vec<String>>,
    pub allowed_extensions: Option<Vec<String>>,
}

impl From<PathAttrSpecRaw> for PathAttrSpec {
    fn from(value: PathAttrSpecRaw) -> Self {
        Self {
            direction: value.direction,
            kind: value.kind,
            allowed_mime_types: value.allowed_mime_types.map(|s| s.0),
            allowed_extensions: value.allowed_extensions.map(|s| s.0),
        }
    }
}

#[derive(Default, Debug, Clone, FromMeta)]
pub struct UrlSpecRaw {
    #[darling(default)]
    pub allowed_schemes: Option<StringList>,
    #[darling(default)]
    pub allowed_hosts: Option<StringList>,
}

#[derive(Default, Debug, Clone)]
pub struct UrlSpec {
    pub allowed_schemes: Option<Vec<String>>,
    pub allowed_hosts: Option<Vec<String>>,
}

impl From<UrlSpecRaw> for UrlSpec {
    fn from(value: UrlSpecRaw) -> Self {
        Self {
            allowed_schemes: value.allowed_schemes.map(|s| s.0),
            allowed_hosts: value.allowed_hosts.map(|s| s.0),
        }
    }
}

#[derive(Debug, Clone, FromMeta)]
pub struct QuantitySpecRaw {
    pub base_unit: String,
    #[darling(default)]
    pub allowed_suffixes: Option<StringList>,
    #[darling(default)]
    pub min: Option<String>,
    #[darling(default)]
    pub max: Option<String>,
}

#[derive(Debug, Clone)]
pub struct QuantitySpec {
    pub base_unit: String,
    pub allowed_suffixes: Option<Vec<String>>,
    pub min: Option<String>,
    pub max: Option<String>,
}

impl From<QuantitySpecRaw> for QuantitySpec {
    fn from(value: QuantitySpecRaw) -> Self {
        Self {
            base_unit: value.base_unit,
            allowed_suffixes: value.allowed_suffixes.map(|s| s.0),
            min: value.min,
            max: value.max,
        }
    }
}

/// Wrapper that lets `darling` parse `["a", "b"]` array literals into
/// `Vec<String>`. The default `Vec<T>` impl in darling expects repeated
/// attribute keys (`foo = "x", foo = "y"`); the explicit array form below is
/// more ergonomic at the call site.
#[derive(Default, Debug, Clone)]
pub struct StringList(pub Vec<String>);

impl FromMeta for StringList {
    fn from_list(items: &[NestedMeta]) -> darling::Result<Self> {
        let mut out = Vec::with_capacity(items.len());
        for item in items {
            match item {
                NestedMeta::Lit(Lit::Str(s)) => out.push(s.value()),
                other => {
                    return Err(darling::Error::custom("expected a list of string literals")
                        .with_span(other));
                }
            }
        }
        Ok(StringList(out))
    }

    fn from_value(value: &Lit) -> darling::Result<Self> {
        match value {
            Lit::Str(s) => Ok(StringList(vec![s.value()])),
            other => Err(darling::Error::custom(
                "expected a string literal or a list of string literals",
            )
            .with_span(other)),
        }
    }
}

#[derive(Default, Debug, Clone, FromMeta)]
pub struct QuotaTokenAttrSpec {
    #[darling(default)]
    pub resource_name: Option<String>,
}

#[derive(Debug, Clone)]
pub enum DiscriminatorAttr {
    Prefix(String),
    Suffix(String),
    Contains(String),
    Regex(String),
    FieldEquals {
        field: String,
        literal: Option<String>,
    },
    FieldAbsent(String),
}

#[derive(Debug, Clone, FromMeta)]
pub struct FieldEqualsRaw {
    pub field: String,
    #[darling(default)]
    pub literal: Option<String>,
}

pub fn parse_type_attrs(attrs: &[Attribute]) -> syn::Result<TypeAttrs> {
    let mut out = TypeAttrs::default();
    for attr in attrs.iter().filter(|a| a.path().is_ident(SCHEMA_ATTR)) {
        let metas = nested_metas(attr)?;
        for meta in metas {
            apply_type_meta(&mut out, &meta)?;
        }
    }
    if out.union && (out.adjacent_tag.is_some() || out.adjacent_content.is_some()) {
        return Err(syn::Error::new(
            proc_macro2::Span::call_site(),
            "`#[schema(union)]` and `#[schema(tag = …, content = …)]` are mutually exclusive",
        ));
    }
    if out.adjacent_tag.is_some() != out.adjacent_content.is_some() {
        return Err(syn::Error::new(
            proc_macro2::Span::call_site(),
            "`#[schema(tag = …)]` and `#[schema(content = …)]` must be used together",
        ));
    }
    if out.adjacent_tag.is_some() || out.adjacent_content.is_some() {
        let span = attrs
            .iter()
            .find(|a| a.path().is_ident(SCHEMA_ATTR))
            .map(|a| a.span())
            .unwrap_or_else(proc_macro2::Span::call_site);
        return Err(syn::Error::new(
            span,
            "#[schema(tag/content)] are parsed but not yet implemented — use the default WIT-style variant emission, or wait for follow-up support",
        ));
    }
    Ok(out)
}

pub fn parse_item_attrs(attrs: &[Attribute]) -> syn::Result<ItemAttrs> {
    let mut out = ItemAttrs::default();
    let mut rich_count = 0usize;
    for attr in attrs.iter().filter(|a| a.path().is_ident(SCHEMA_ATTR)) {
        let metas = nested_metas(attr)?;
        for meta in metas {
            apply_item_meta(&mut out, &meta, &mut rich_count)?;
        }
    }
    if rich_count > 1 {
        return Err(syn::Error::new_spanned(
            attrs
                .iter()
                .find(|a| a.path().is_ident(SCHEMA_ATTR))
                .map(|a| a as &dyn quote::ToTokens)
                .unwrap_or(&""),
            "conflicting rich-scalar attributes on the same item (only one of text/binary/path/url/quantity/quota_token allowed)",
        ));
    }
    if out.flatten {
        let span = attrs
            .iter()
            .find(|a| a.path().is_ident(SCHEMA_ATTR))
            .map(|a| a.span())
            .unwrap_or_else(proc_macro2::Span::call_site);
        return Err(syn::Error::new(
            span,
            "#[schema(flatten)] is parsed but not yet implemented — remove the attribute or wait for follow-up support",
        ));
    }
    Ok(out)
}

fn nested_metas(attr: &Attribute) -> syn::Result<Vec<NestedMeta>> {
    let meta = attr.meta.clone();
    match meta {
        Meta::List(list) => {
            let tokens: proc_macro2::TokenStream = list.tokens;
            NestedMeta::parse_meta_list(tokens)
        }
        Meta::Path(path) => Err(syn::Error::new_spanned(path, "expected `#[schema(...)]`")),
        Meta::NameValue(nv) => Err(syn::Error::new_spanned(nv, "expected `#[schema(...)]`")),
    }
}

fn apply_type_meta(out: &mut TypeAttrs, meta: &NestedMeta) -> syn::Result<()> {
    match meta {
        NestedMeta::Meta(Meta::Path(path)) => {
            if path.is_ident("deprecated") {
                out.deprecated = Some(DeprecatedMarker::Flag);
            } else if path.is_ident("union") {
                out.union = true;
            } else if path.is_ident("transparent") {
                out.transparent = true;
            } else {
                return Err(syn::Error::new_spanned(
                    path,
                    format!(
                        "unknown schema attribute `{}` on a type",
                        path_ident_string(path)
                    ),
                ));
            }
        }
        NestedMeta::Meta(Meta::NameValue(nv)) => {
            let name = nv
                .path
                .get_ident()
                .map(|i| i.to_string())
                .unwrap_or_default();
            let value = lit_to_string(&nv.value)?;
            match name.as_str() {
                "named" => out.named = Some(value),
                "doc" => out.doc = Some(value),
                "alias" => out.alias.push(value),
                "example" => out.example.push(value),
                "deprecated" => out.deprecated = Some(DeprecatedMarker::Message(value)),
                "role" => out.role = Some(value),
                "rename_all" => {
                    out.rename_all = RenameAll::from_string(&value, nv.span())?;
                }
                "tag" => out.adjacent_tag = Some(value),
                "content" => out.adjacent_content = Some(value),
                _ => {
                    return Err(syn::Error::new_spanned(
                        nv,
                        format!("unknown schema attribute `{name}` on a type"),
                    ));
                }
            }
        }
        other => {
            return Err(syn::Error::new_spanned(
                meta_tokens(other),
                "unsupported schema attribute form on a type",
            ));
        }
    }
    Ok(())
}

fn apply_item_meta(
    out: &mut ItemAttrs,
    meta: &NestedMeta,
    rich_count: &mut usize,
) -> syn::Result<()> {
    match meta {
        NestedMeta::Meta(Meta::Path(path)) => {
            if path.is_ident("deprecated") {
                out.deprecated = Some(DeprecatedMarker::Flag);
            } else if path.is_ident("quota_token") {
                *rich_count += 1;
                out.rich = Some(RichSpec::QuotaToken(QuotaTokenAttrSpec::default()));
            } else if path.is_ident("skip") {
                out.skip = true;
            } else if path.is_ident("flatten") {
                out.flatten = true;
            } else {
                return Err(syn::Error::new_spanned(
                    path,
                    format!(
                        "unknown schema attribute `{}` on a field or case",
                        path_ident_string(path)
                    ),
                ));
            }
        }
        NestedMeta::Meta(Meta::NameValue(nv)) => {
            let name = nv
                .path
                .get_ident()
                .map(|i| i.to_string())
                .unwrap_or_default();
            let value = lit_to_string(&nv.value)?;
            match name.as_str() {
                "rename" => out.rename = Some(value),
                "doc" => out.doc = Some(value),
                "alias" => out.alias.push(value),
                "example" => out.example.push(value),
                "deprecated" => out.deprecated = Some(DeprecatedMarker::Message(value)),
                "source" => out.source = Some(value),
                "kind" => out.source_kind = Some(value),
                "default" => out.default_with = Some(value),
                "prefix" => {
                    set_disc(out, DiscriminatorAttr::Prefix(value), nv)?;
                }
                "suffix" => {
                    set_disc(out, DiscriminatorAttr::Suffix(value), nv)?;
                }
                "contains" => {
                    set_disc(out, DiscriminatorAttr::Contains(value), nv)?;
                }
                "regex" => {
                    set_disc(out, DiscriminatorAttr::Regex(value), nv)?;
                }
                "field_absent" => {
                    set_disc(out, DiscriminatorAttr::FieldAbsent(value), nv)?;
                }
                _ => {
                    return Err(syn::Error::new_spanned(
                        nv,
                        format!("unknown schema attribute `{name}` on a field or case"),
                    ));
                }
            }
        }
        NestedMeta::Meta(Meta::List(list)) => {
            let name = list
                .path
                .get_ident()
                .map(|i| i.to_string())
                .unwrap_or_default();
            let nested = nested_metas_from_list(list)?;
            match name.as_str() {
                "text" => {
                    *rich_count += 1;
                    let spec = TextSpecRaw::from_list(&nested)?;
                    out.rich = Some(RichSpec::Text(spec.into()));
                }
                "binary" => {
                    *rich_count += 1;
                    let spec = BinarySpecRaw::from_list(&nested)?;
                    out.rich = Some(RichSpec::Binary(spec.into()));
                }
                "path" => {
                    *rich_count += 1;
                    let spec = PathAttrSpecRaw::from_list(&nested)?;
                    out.rich = Some(RichSpec::Path(spec.into()));
                }
                "url" => {
                    *rich_count += 1;
                    let spec = UrlSpecRaw::from_list(&nested)?;
                    out.rich = Some(RichSpec::Url(spec.into()));
                }
                "quantity" => {
                    *rich_count += 1;
                    let spec = QuantitySpecRaw::from_list(&nested)?;
                    out.rich = Some(RichSpec::Quantity(spec.into()));
                }
                "quota_token" => {
                    *rich_count += 1;
                    let spec = QuotaTokenAttrSpec::from_list(&nested)?;
                    out.rich = Some(RichSpec::QuotaToken(spec));
                }
                "field_equals" => {
                    let raw = FieldEqualsRaw::from_list(&nested)?;
                    set_disc(
                        out,
                        DiscriminatorAttr::FieldEquals {
                            field: raw.field,
                            literal: raw.literal,
                        },
                        list,
                    )?;
                }
                _ => {
                    return Err(syn::Error::new_spanned(
                        list,
                        format!("unknown schema attribute `{name}` on a field or case"),
                    ));
                }
            }
        }
        other => {
            return Err(syn::Error::new_spanned(
                meta_tokens(other),
                "unsupported schema attribute form on a field or case",
            ));
        }
    }
    Ok(())
}

fn set_disc<S: quote::ToTokens>(
    out: &mut ItemAttrs,
    disc: DiscriminatorAttr,
    span_src: S,
) -> syn::Result<()> {
    if out.discriminator.is_some() {
        return Err(syn::Error::new_spanned(
            span_src,
            "a union branch may declare at most one discriminator",
        ));
    }
    out.discriminator = Some(disc);
    Ok(())
}

fn nested_metas_from_list(list: &syn::MetaList) -> syn::Result<Vec<NestedMeta>> {
    let tokens = list.tokens.clone();
    NestedMeta::parse_meta_list(tokens)
}

fn lit_to_string(expr: &syn::Expr) -> syn::Result<String> {
    if let syn::Expr::Lit(lit) = expr {
        match &lit.lit {
            Lit::Str(s) => Ok(s.value()),
            Lit::Int(i) => Ok(i.base10_digits().to_string()),
            Lit::Bool(b) => Ok(b.value.to_string()),
            other => Err(syn::Error::new_spanned(other, "expected a string literal")),
        }
    } else {
        Err(syn::Error::new_spanned(
            expr,
            "expected a literal expression",
        ))
    }
}

fn path_ident_string(path: &Path) -> String {
    path.segments
        .iter()
        .map(|s| s.ident.to_string())
        .collect::<Vec<_>>()
        .join("::")
}

fn meta_tokens(meta: &NestedMeta) -> proc_macro2::TokenStream {
    quote::quote!(#meta)
}
