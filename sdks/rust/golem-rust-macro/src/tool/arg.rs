// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Parser for the `#[arg(...)]` helper attribute.

use crate::tool::helpers::{
    SeenKeys, expr_bool, expr_char, expr_str, expr_str_array, expr_u32, parse_attr_exprs,
    require_metadata_literal,
};
use crate::tool::ir::{
    ArgIr, ArgPlacement, ArgSubKind, PathDirectionIr, PathKindIr, RepeatableMode,
};
use syn::spanned::Spanned;
use syn::{Attribute, Error, Expr};

/// Parses a single `#[arg(...)]` attribute into an [`ArgIr`].
pub fn parse_arg(attr: &Attribute) -> Result<ArgIr, Error> {
    let exprs = parse_attr_exprs(attr)?;
    let mut iter = exprs.iter();

    let head = iter.next().ok_or_else(|| {
        Error::new(
            attr.span(),
            "#[arg(...)] must start with a parameter name, e.g. `#[arg(name = \"positional\")]`",
        )
    })?;
    let (param, placement) = parse_head(head)?;
    let mut ir = ArgIr::new(param);
    ir.placement = placement;

    // Only the boolean options have a bare form (`verbatim` == `verbatim = true`);
    // every other key requires `key = value`.
    const BARE_OK: [&str; 5] = [
        "required",
        "negatable",
        "optional_scalar",
        "verbatim",
        "accepts_stdio",
    ];
    let mut seen = SeenKeys::default();
    for expr in iter {
        let (key, value, is_bare) = split_key_value(expr)?;
        seen.insert(&key)?;
        let key_str = key.to_string();
        if is_bare && !BARE_OK.contains(&key_str.as_str()) {
            return Err(Error::new(
                key.span(),
                format!(
                    "`{key_str}` does not take a bare form; expected `key = value` in #[arg(...)]"
                ),
            ));
        }
        match key_str.as_str() {
            "kind" => apply_kind(&mut ir, value)?,
            "short" => ir.short = Some(expr_char(value, "short")?),
            "aliases" => ir.aliases = expr_str_array(value, "aliases")?,
            "env" => ir.env = Some(expr_str(value, "env")?),
            "required" => ir.required = Some(value_bool(value, is_bare, "required")?),
            "negatable" => ir.negatable = Some(value_bool(value, is_bare, "negatable")?),
            "optional_scalar" => {
                ir.optional_scalar = value_bool(value, is_bare, "optional_scalar")?
            }
            "repeatable" => ir.repeatable = Some(parse_repeatable(value)?),
            "delim" => ir.delim = Some(expr_char(value, "delim")?),
            "default" => {
                require_metadata_literal(value, "default")?;
                ir.default = Some(value.clone());
            }
            "separator" => ir.separator = Some(expr_str(value, "separator")?),
            "verbatim" => ir.verbatim = value_bool(value, is_bare, "verbatim")?,
            "accepts_stdio" => ir.accepts_stdio = value_bool(value, is_bare, "accepts_stdio")?,
            "regex" => ir.regex = Some(expr_str(value, "regex")?),
            "min_length" => ir.min_length = Some(expr_u32(value, "min_length")?),
            "max_length" => ir.max_length = Some(expr_u32(value, "max_length")?),
            "direction" => ir.direction = Some(parse_direction(value)?),
            "mime" => ir.mime = Some(expr_str_array(value, "mime")?),
            "schemes" => ir.schemes = Some(expr_str_array(value, "schemes")?),
            "unit" => ir.unit = Some(expr_str(value, "unit")?),
            "bounds" => ir.bounds = Some(parse_bounds(value)?),
            "doc" => ir.doc = Some(expr_str(value, "doc")?),
            "value_name" => ir.value_name = Some(expr_str(value, "value_name")?),
            // `min`/`max` are kept raw; their meaning (tail occurrence count,
            // count-flag max, or numeric bound) depends on the final placement
            // and sub-kind, which metadata synthesis resolves (placement may be inferred
            // from the parameter type).
            "min" => ir.raw_min = Some(value.clone()),
            "max" => ir.raw_max = Some(value.clone()),
            other => {
                return Err(Error::new(
                    key.span(),
                    format!("unknown #[arg] key `{other}`"),
                ));
            }
        }
    }

    Ok(ir)
}

/// Parses the leading entry: either a bare `param` or `param = "<placement>"`.
fn parse_head(expr: &Expr) -> Result<(syn::Ident, Option<ArgPlacement>), Error> {
    match expr {
        Expr::Path(p) => {
            let ident = p.path.get_ident().cloned().ok_or_else(|| {
                Error::new(
                    p.span(),
                    "expected a parameter name as the first #[arg] entry",
                )
            })?;
            Ok((ident, None))
        }
        Expr::Assign(assign) => {
            let ident = assign_left_ident(&assign.left)?;
            let placement =
                parse_placement(&expr_str(&assign.right, "placement")?, assign.right.span())?;
            Ok((ident, Some(placement)))
        }
        other => Err(Error::new(
            other.span(),
            "the first #[arg] entry must be `<param>` or `<param> = \"<placement>\"`",
        )),
    }
}

fn parse_placement(value: &str, span: proc_macro2::Span) -> Result<ArgPlacement, Error> {
    match value {
        "global" => Ok(ArgPlacement::Global),
        "positional" => Ok(ArgPlacement::Positional),
        "option" => Ok(ArgPlacement::Option),
        "flag" => Ok(ArgPlacement::Flag),
        "tail" => Ok(ArgPlacement::Tail),
        other => Err(Error::new(
            span,
            format!(
                "invalid arg placement `{other}`; expected one of: global, positional, option, flag, tail"
            ),
        )),
    }
}

fn apply_kind(ir: &mut ArgIr, value: &Expr) -> Result<(), Error> {
    let s = expr_str(value, "kind")?;
    match s.as_str() {
        "flag" => ir.sub_kind = Some(ArgSubKind::Flag),
        "count-flag" => ir.sub_kind = Some(ArgSubKind::CountFlag),
        "file" => ir.path_kind = Some(PathKindIr::File),
        "dir" | "directory" => ir.path_kind = Some(PathKindIr::Directory),
        "any" => ir.path_kind = Some(PathKindIr::Any),
        other => {
            return Err(Error::new(
                value.span(),
                format!(
                    "invalid kind `{other}`; expected one of: flag, count-flag (arg kind) or file, dir, any (path kind)"
                ),
            ));
        }
    }
    Ok(())
}

fn parse_repeatable(value: &Expr) -> Result<RepeatableMode, Error> {
    let s = expr_str(value, "repeatable")?;
    match s.as_str() {
        "repeated" => Ok(RepeatableMode::Repeated),
        "delimited" => Ok(RepeatableMode::Delimited),
        "either" => Ok(RepeatableMode::Either),
        other => Err(Error::new(
            value.span(),
            format!("invalid repeatable mode `{other}`; expected: repeated, delimited, either"),
        )),
    }
}

fn parse_direction(value: &Expr) -> Result<PathDirectionIr, Error> {
    let s = expr_str(value, "direction")?;
    match s.as_str() {
        "input" | "in" => Ok(PathDirectionIr::Input),
        "output" | "out" => Ok(PathDirectionIr::Output),
        "inout" | "in-out" | "in_out" => Ok(PathDirectionIr::InOut),
        other => Err(Error::new(
            value.span(),
            format!("invalid direction `{other}`; expected: input, output, inout"),
        )),
    }
}

fn parse_bounds(value: &Expr) -> Result<(Expr, Expr), Error> {
    match value {
        Expr::Tuple(t) if t.elems.len() == 2 => {
            let mut it = t.elems.iter();
            let lo = it.next().unwrap().clone();
            let hi = it.next().unwrap().clone();
            Ok((lo, hi))
        }
        other => Err(Error::new(
            other.span(),
            "bounds must be a 2-tuple `(min, max)`",
        )),
    }
}

/// Reads a key-value entry. The returned flag is `true` when the entry was a
/// bare `key` path (no `= value`), which the boolean handlers treat as `true`.
fn split_key_value(expr: &Expr) -> Result<(syn::Ident, &Expr, bool), Error> {
    match expr {
        Expr::Assign(assign) => {
            let ident = assign_left_ident(&assign.left)?;
            Ok((ident, &assign.right, false))
        }
        Expr::Path(p) => {
            let ident =
                p.path.get_ident().cloned().ok_or_else(|| {
                    Error::new(p.span(), "expected a key identifier in #[arg(...)]")
                })?;
            // A bare key (e.g. `verbatim`) means the key set to `true`; the
            // expression itself is reused as the value, which only the boolean
            // handlers accept (and only because `is_bare` is `true`).
            Ok((ident, expr, true))
        }
        other => Err(Error::new(
            other.span(),
            "expected `key = value` in #[arg(...)]",
        )),
    }
}

/// Resolves a boolean key: a bare key is `true`; an assigned key requires a
/// boolean literal (`= true` / `= false`), rejecting anything else.
fn value_bool(value: &Expr, is_bare: bool, what: &str) -> Result<bool, Error> {
    if is_bare {
        return Ok(true);
    }
    expr_bool(value, what)
}

fn assign_left_ident(left: &Expr) -> Result<syn::Ident, Error> {
    if let Expr::Path(p) = left
        && let Some(ident) = p.path.get_ident()
    {
        return Ok(ident.clone());
    }
    Err(Error::new(
        left.span(),
        "expected an identifier on the left of `=`",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arg(src: &str) -> Result<ArgIr, Error> {
        let item: syn::ItemStruct = syn::parse_str(&format!("{src}\nstruct S;")).unwrap();
        parse_arg(&item.attrs[0])
    }

    #[test]
    fn positional_with_regex() {
        let ir = arg(r##"#[arg(pattern = "positional", regex = r"^.+$")]"##).unwrap();
        assert_eq!(ir.param.to_string(), "pattern");
        assert_eq!(ir.placement, Some(ArgPlacement::Positional));
        assert_eq!(ir.regex.as_deref(), Some("^.+$"));
    }

    #[test]
    fn global_flag() {
        let ir = arg(r#"#[arg(case_sensitive = "global", short = 'i', kind = "flag")]"#).unwrap();
        assert_eq!(ir.param.to_string(), "case_sensitive");
        assert_eq!(ir.placement, Some(ArgPlacement::Global));
        assert_eq!(ir.short, Some('i'));
        assert_eq!(ir.sub_kind, Some(ArgSubKind::Flag));
    }

    #[test]
    fn repeatable_either_with_delim() {
        let ir = arg(
            r#"#[arg(extra_patterns = "option", short = 'e', repeatable = "either", delim = ',')]"#,
        )
        .unwrap();
        assert_eq!(ir.repeatable, Some(RepeatableMode::Either));
        assert_eq!(ir.delim, Some(','));
    }

    #[test]
    fn min_max_kept_raw() {
        // `min`/`max` are captured raw regardless of placement; metadata synthesis routes
        // them once the final placement/sub-kind is known.
        let ir = arg(r#"#[arg(max_count = "option", short = 'n', min = 1)]"#).unwrap();
        assert!(ir.raw_min.is_some());
        assert!(ir.raw_max.is_none());
    }

    #[test]
    fn bare_min_key_is_rejected() {
        let err = arg(r#"#[arg(max_count = "option", short = 'n', min)]"#).unwrap_err();
        assert!(err.to_string().contains("expected `key = value`"));
    }

    #[test]
    fn tail_min_is_raw() {
        let ir = arg(r#"#[arg(paths = "tail", separator = "--", min = 0)]"#).unwrap();
        assert_eq!(ir.placement, Some(ArgPlacement::Tail));
        assert!(ir.raw_min.is_some());
        assert_eq!(ir.separator.as_deref(), Some("--"));
    }

    #[test]
    fn count_flag_max_is_raw() {
        let ir = arg(r#"#[arg(verbose = "global", short = 'v', kind = "count-flag", max = 3)]"#)
            .unwrap();
        assert_eq!(ir.sub_kind, Some(ArgSubKind::CountFlag));
        assert!(ir.raw_max.is_some());
    }

    #[test]
    fn numeric_bounds_tuple() {
        let ir =
            arg(r#"#[arg(max_count = "option", short = 'n', bounds = (0, i64::MAX))]"#).unwrap();
        assert!(ir.bounds.is_some());
    }

    #[test]
    fn accepts_stdio_on_tail() {
        let ir = arg(r#"#[arg(files = "tail", accepts_stdio = true)]"#).unwrap();
        assert_eq!(ir.placement, Some(ArgPlacement::Tail));
        assert!(ir.accepts_stdio);
    }

    #[test]
    fn env_default_required() {
        let ir = arg(r#"#[arg(message = "option", short = 'm', required = true)]"#).unwrap();
        assert_eq!(ir.required, Some(true));
        let ir = arg(r#"#[arg(git_dir = "global", env = "GIT_DIR", default = ".git")]"#).unwrap();
        assert_eq!(ir.env.as_deref(), Some("GIT_DIR"));
        assert!(ir.default.is_some());
    }

    #[test]
    fn negatable_flag_default_true() {
        let ir =
            arg(r#"#[arg(paginate = "global", kind = "flag", negatable = true, default = true)]"#)
                .unwrap();
        assert_eq!(ir.negatable, Some(true));
        assert!(ir.default.is_some());
    }

    #[test]
    fn bare_param_infers_placement() {
        let ir = arg(r#"#[arg(pattern, regex = r"^.+$")]"#).unwrap();
        assert_eq!(ir.param.to_string(), "pattern");
        assert_eq!(ir.placement, None);
    }

    #[test]
    fn path_kind_via_kind_key() {
        let ir = arg(r#"#[arg(out = "positional", kind = "file", direction = "output")]"#).unwrap();
        assert_eq!(ir.path_kind, Some(PathKindIr::File));
        assert_eq!(ir.direction, Some(PathDirectionIr::Output));
        assert!(ir.sub_kind.is_none());
    }

    #[test]
    fn url_schemes() {
        let ir = arg(r#"#[arg(endpoint = "positional", schemes = ["https", "http"])]"#).unwrap();
        assert_eq!(
            ir.schemes,
            Some(vec!["https".to_string(), "http".to_string()])
        );
    }

    #[test]
    fn unknown_key_is_error() {
        let err = arg(r#"#[arg(name = "option", bogus = 1)]"#).unwrap_err();
        assert!(err.to_string().contains("unknown #[arg] key"));
    }

    #[test]
    fn assigned_bool_must_be_boolean_literal() {
        let err = arg(r#"#[arg(name = "option", required = maybe)]"#).unwrap_err();
        assert!(err.to_string().contains("required"));
    }

    #[test]
    fn default_rejects_negated_string_literal() {
        let err = arg(r#"#[arg(name = "option", default = -"bad")]"#).unwrap_err();
        assert!(err.to_string().contains("literal"));
    }

    #[test]
    fn default_rejects_byte_string_literal() {
        assert!(
            arg(r#"#[arg(name = "option", default = b"bad")]"#).is_err(),
            "byte string defaults are accepted into the IR even though metadata literals only support strings, numbers, bools, chars, arrays, and tuples"
        );
    }

    #[test]
    fn duplicate_arg_key_is_error() {
        let err = arg(r#"#[arg(name = "option", short = 'n', short = 'm')]"#).unwrap_err();
        assert!(err.to_string().contains("duplicate"));
    }

    #[test]
    fn invalid_placement_is_error() {
        let err = arg(r#"#[arg(name = "weird")]"#).unwrap_err();
        assert!(err.to_string().contains("invalid arg placement"));
    }
}
