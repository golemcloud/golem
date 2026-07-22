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

//! Parser for the `#[constraint(...)]` helper attribute.

use crate::tool::helpers::{expr_str, parse_attr_exprs, require_metadata_literal};
use crate::tool::ir::{ConstraintIr, QuantifierIr, RefIr};
use syn::spanned::Spanned;
use syn::{Attribute, Error, Expr};

/// Parses a single `#[constraint(...)]` attribute into one [`ConstraintIr`].
pub fn parse_constraint(attr: &Attribute) -> Result<ConstraintIr, Error> {
    let exprs = parse_attr_exprs(attr)?;
    if exprs.len() != 1 {
        return Err(Error::new(
            attr.span(),
            "#[constraint(...)] must contain exactly one constraint, e.g. `all_or_none = [...]` or `implies(...)`",
        ));
    }
    let expr = exprs.first().unwrap();
    match expr {
        Expr::Assign(assign) => {
            let key = assign_left_ident(&assign.left)?;
            match key.to_string().as_str() {
                "requires_all" => Ok(ConstraintIr::RequiresAll(parse_refs(&assign.right)?)),
                "all_or_none" => Ok(ConstraintIr::AllOrNone(parse_refs(&assign.right)?)),
                "requires_any" => Ok(ConstraintIr::RequiresAny(parse_refs(&assign.right)?)),
                "mutex_groups" => Ok(ConstraintIr::MutexGroups(parse_ref_groups(&assign.right)?)),
                other => Err(Error::new(
                    key.span(),
                    format!("unknown #[constraint] key `{other}`"),
                )),
            }
        }
        Expr::Call(call) => {
            let func = func_ident(&call.func)?;
            match func.to_string().as_str() {
                "implies" => parse_implies(call),
                "forbids" => parse_forbids(call),
                other => Err(Error::new(
                    call.func.span(),
                    format!("unknown #[constraint] form `{other}(...)`"),
                )),
            }
        }
        other => Err(Error::new(
            other.span(),
            "expected a constraint like `all_or_none = [...]`, `implies(...)`, or `forbids(...)`",
        )),
    }
}

fn parse_implies(call: &syn::ExprCall) -> Result<ConstraintIr, Error> {
    let kw = collect_kwargs(call)?;
    check_allowed_keys(&kw, &["lhs", "rhs", "lhs_quant", "rhs_quant"])?;
    let (lhs_quant, lhs) = take_side(&kw, "lhs")?;
    let (rhs_quant, rhs) = take_side(&kw, "rhs")?;
    Ok(ConstraintIr::Implies {
        lhs_quant,
        lhs,
        rhs_quant,
        rhs,
    })
}

fn parse_forbids(call: &syn::ExprCall) -> Result<ConstraintIr, Error> {
    let kw = collect_kwargs(call)?;
    // `forbids` has no RHS quantifier in the runtime model, so reject `rhs_quant`.
    check_allowed_keys(&kw, &["lhs", "rhs", "lhs_quant"])?;
    let (lhs_quant, lhs) = take_side(&kw, "lhs")?;
    let (_rhs_quant, rhs) = take_side(&kw, "rhs")?;
    Ok(ConstraintIr::Forbids {
        lhs_quant,
        lhs,
        rhs,
    })
}

/// Rejects unknown or duplicated keyword arguments in a constraint form.
fn check_allowed_keys(kw: &[(syn::Ident, Expr)], allowed: &[&str]) -> Result<(), Error> {
    let mut seen = std::collections::BTreeSet::new();
    for (key, _) in kw {
        let name = key.to_string();
        if !allowed.contains(&name.as_str()) {
            return Err(Error::new(
                key.span(),
                format!(
                    "unknown key `{name}`; expected one of: {}",
                    allowed.join(", ")
                ),
            ));
        }
        if !seen.insert(name.clone()) {
            return Err(Error::new(key.span(), format!("duplicate key `{name}`")));
        }
    }
    Ok(())
}

/// Resolves a `lhs`/`rhs` side: refs from `<side>` and an optional
/// `<side>_quant = "all" | "any"` (defaulting to `all`).
fn take_side(kw: &[(syn::Ident, Expr)], side: &str) -> Result<(QuantifierIr, Vec<RefIr>), Error> {
    let refs_expr = kw
        .iter()
        .find(|(k, _)| k == side)
        .map(|(_, v)| v)
        .ok_or_else(|| Error::new(proc_macro2::Span::call_site(), format!("missing `{side}`")))?;
    let refs = parse_refs(refs_expr)?;
    let quant = match kw.iter().find(|(k, _)| *k == format!("{side}_quant")) {
        Some((_, v)) => parse_quantifier(v)?,
        None => QuantifierIr::All,
    };
    Ok((quant, refs))
}

fn parse_quantifier(expr: &Expr) -> Result<QuantifierIr, Error> {
    match expr_str(expr, "quantifier")?.as_str() {
        "all" => Ok(QuantifierIr::All),
        "any" => Ok(QuantifierIr::Any),
        other => Err(Error::new(
            expr.span(),
            format!("invalid quantifier `{other}`; expected `all` or `any`"),
        )),
    }
}

fn collect_kwargs(call: &syn::ExprCall) -> Result<Vec<(syn::Ident, Expr)>, Error> {
    let mut out = Vec::new();
    for arg in call.args.iter() {
        let Expr::Assign(assign) = arg else {
            return Err(Error::new(
                arg.span(),
                "expected `key = value` arguments (e.g. `lhs = \"a\", rhs = \"b\"`)",
            ));
        };
        out.push((assign_left_ident(&assign.left)?, (*assign.right).clone()));
    }
    Ok(out)
}

/// Parses a flexible ref list: either a single ref or an array of refs.
fn parse_refs(expr: &Expr) -> Result<Vec<RefIr>, Error> {
    match expr {
        Expr::Array(arr) => arr.elems.iter().map(parse_ref).collect(),
        other => Ok(vec![parse_ref(other)?]),
    }
}

fn parse_ref_groups(expr: &Expr) -> Result<Vec<Vec<RefIr>>, Error> {
    let Expr::Array(arr) = expr else {
        return Err(Error::new(
            expr.span(),
            "mutex_groups must be an array of groups, e.g. `[[\"add\"], [\"delete\"]]`",
        ));
    };
    // Each group must itself be an array; a bare ref like `mutex_groups =
    // [\"add\", \"delete\"]` would otherwise be silently parsed as two no-op
    // single-element groups instead of one mutually-exclusive group.
    arr.elems
        .iter()
        .map(|group| match group {
            Expr::Array(group_arr) => group_arr.elems.iter().map(parse_ref).collect(),
            other => Err(Error::new(
                other.span(),
                "mutex_groups must be an array of groups, e.g. `[[\"add\"], [\"delete\"]]`; each group must itself be an array of argument refs",
            )),
        })
        .collect()
}

/// Parses a single ref: a string literal (`present`) or `value_is(...)`.
fn parse_ref(expr: &Expr) -> Result<RefIr, Error> {
    match expr {
        Expr::Lit(_) => Ok(RefIr::Present(expr_str(expr, "argument name")?)),
        Expr::Call(call)
            if func_ident(&call.func)
                .map(|i| i == "value_is")
                .unwrap_or(false) =>
        {
            parse_value_is(call)
        }
        other => Err(Error::new(
            other.span(),
            "expected an argument name string or `value_is(name, value)`",
        )),
    }
}

/// Parses `value_is("name", <literal>)` or `value_is(name = "name", value = <literal>)`.
fn parse_value_is(call: &syn::ExprCall) -> Result<RefIr, Error> {
    let args: Vec<&Expr> = call.args.iter().collect();
    let any_keyword = args.iter().any(|a| matches!(a, Expr::Assign(_)));
    let all_keyword = args.iter().all(|a| matches!(a, Expr::Assign(_)));
    if any_keyword && !all_keyword {
        return Err(Error::new(
            call.span(),
            "value_is(...) cannot mix positional and keyword arguments; use `value_is(\"name\", <value>)` or `value_is(name = \"name\", value = <value>)`",
        ));
    }
    // Keyword form.
    if all_keyword && !args.is_empty() {
        let kw = collect_kwargs(call)?;
        check_allowed_keys(&kw, &["name", "value"])?;
        let name = kw
            .iter()
            .find(|(k, _)| k == "name")
            .ok_or_else(|| Error::new(call.span(), "value_is(...) is missing `name`"))?;
        let value = kw
            .iter()
            .find(|(k, _)| k == "value")
            .ok_or_else(|| Error::new(call.span(), "value_is(...) is missing `value`"))?;
        require_metadata_literal(&value.1, "value_is value")?;
        return Ok(RefIr::ValueIs {
            name: expr_str(&name.1, "value_is name")?,
            value: value.1.clone(),
        });
    }
    // Positional form: value_is("name", <literal>).
    if args.len() == 2 {
        require_metadata_literal(args[1], "value_is value")?;
        return Ok(RefIr::ValueIs {
            name: expr_str(args[0], "value_is name")?,
            value: args[1].clone(),
        });
    }
    Err(Error::new(
        call.span(),
        "value_is(...) takes `name` and `value` (positional or keyword)",
    ))
}

fn func_ident(func: &Expr) -> Result<syn::Ident, Error> {
    if let Expr::Path(p) = func
        && let Some(ident) = p.path.get_ident()
    {
        return Ok(ident.clone());
    }
    Err(Error::new(func.span(), "expected an identifier"))
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

    fn constraint(src: &str) -> Result<ConstraintIr, Error> {
        let item: syn::ItemStruct = syn::parse_str(&format!("{src}\nstruct S;")).unwrap();
        parse_constraint(&item.attrs[0])
    }

    #[test]
    fn all_or_none() {
        let c = constraint(r#"#[constraint(all_or_none = ["all-match", "grep"])]"#).unwrap();
        assert_eq!(
            c,
            ConstraintIr::AllOrNone(vec![
                RefIr::Present("all-match".to_string()),
                RefIr::Present("grep".to_string()),
            ])
        );
    }

    #[test]
    fn mutex_groups() {
        let c = constraint(r#"#[constraint(mutex_groups = [["add"], ["delete"]])]"#).unwrap();
        assert_eq!(
            c,
            ConstraintIr::MutexGroups(vec![
                vec![RefIr::Present("add".to_string())],
                vec![RefIr::Present("delete".to_string())],
            ])
        );
    }

    #[test]
    fn mutex_groups_requires_array_of_groups() {
        let err = constraint(r#"#[constraint(mutex_groups = ["add", "delete"])]"#).unwrap_err();
        assert!(
            err.to_string()
                .contains("mutex_groups must be an array of groups")
        );
    }

    #[test]
    fn implies_single() {
        let c =
            constraint(r#"#[constraint(implies(lhs = "reset-author", rhs = "amend"))]"#).unwrap();
        assert_eq!(
            c,
            ConstraintIr::Implies {
                lhs_quant: QuantifierIr::All,
                lhs: vec![RefIr::Present("reset-author".to_string())],
                rhs_quant: QuantifierIr::All,
                rhs: vec![RefIr::Present("amend".to_string())],
            }
        );
    }

    #[test]
    fn implies_with_quantifiers() {
        let c =
            constraint(r#"#[constraint(implies(lhs = ["a", "b"], lhs_quant = "any", rhs = "c"))]"#)
                .unwrap();
        match c {
            ConstraintIr::Implies { lhs_quant, lhs, .. } => {
                assert_eq!(lhs_quant, QuantifierIr::Any);
                assert_eq!(lhs.len(), 2);
            }
            _ => panic!("expected implies"),
        }
    }

    #[test]
    fn forbids() {
        let c = constraint(r#"#[constraint(forbids(lhs = "a", rhs = "b"))]"#).unwrap();
        assert!(matches!(c, ConstraintIr::Forbids { .. }));
    }

    #[test]
    fn value_is_ref_keyword() {
        let c = constraint(
            r#"#[constraint(requires_all = [value_is(name = "output", value = "json")])]"#,
        )
        .unwrap();
        match c {
            ConstraintIr::RequiresAll(refs) => match &refs[0] {
                RefIr::ValueIs { name, .. } => assert_eq!(name, "output"),
                _ => panic!("expected value_is ref"),
            },
            _ => panic!("expected requires_all"),
        }
    }

    #[test]
    fn value_is_ref_positional() {
        let c =
            constraint(r#"#[constraint(requires_any = [value_is("output", "json"), "verbose"])]"#)
                .unwrap();
        match c {
            ConstraintIr::RequiresAny(refs) => {
                assert!(matches!(refs[0], RefIr::ValueIs { .. }));
                assert_eq!(refs[1], RefIr::Present("verbose".to_string()));
            }
            _ => panic!("expected requires_any"),
        }
    }

    #[test]
    fn value_is_rejects_mixed_positional_and_keyword_args() {
        let err =
            constraint(r#"#[constraint(requires_all = [value_is("output", value = "json")])]"#)
                .unwrap_err();
        assert!(err.to_string().contains("value_is"));
    }

    #[test]
    fn value_is_rejects_non_literal_value() {
        let err = constraint(r#"#[constraint(requires_all = [value_is("output", compute())])]"#)
            .unwrap_err();
        assert!(err.to_string().contains("literal"));
    }

    #[test]
    fn value_is_rejects_negated_bool_literal() {
        let err =
            constraint(r#"#[constraint(requires_all = [value_is("output", -true)])]"#).unwrap_err();
        assert!(err.to_string().contains("literal"));
    }

    #[test]
    fn unknown_constraint_is_error() {
        let err = constraint(r#"#[constraint(bogus = ["x"])]"#).unwrap_err();
        assert!(err.to_string().contains("unknown #[constraint] key"));
    }

    #[test]
    fn implies_typo_key_is_error() {
        let err = constraint(r#"#[constraint(implies(lhs = "a", rhs = "b", lhs_qunat = "any"))]"#)
            .unwrap_err();
        assert!(err.to_string().contains("unknown key"));
    }

    #[test]
    fn forbids_rhs_quant_is_rejected() {
        let err = constraint(r#"#[constraint(forbids(lhs = "a", rhs = "b", rhs_quant = "any"))]"#)
            .unwrap_err();
        assert!(err.to_string().contains("unknown key"));
    }
}
