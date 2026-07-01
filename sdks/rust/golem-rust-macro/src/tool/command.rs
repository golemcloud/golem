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

//! Parser for the `#[command(...)]` helper attribute.

use crate::tool::helpers::{SeenKeys, expr_bool, expr_str, expr_str_array, parse_attr_exprs};
use crate::tool::ir::{CommandAnnotationsIr, SubtreeIr};
use syn::spanned::Spanned;
use syn::{Attribute, Error, Expr};

/// The parsed pieces of one or more `#[command(...)]` attributes on a method.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommandAttr {
    pub aliases: Vec<String>,
    pub name_override: Option<String>,
    pub annotations: Option<CommandAnnotationsIr>,
    pub subtree: Option<SubtreeIr>,
}

/// Parses a single `#[command(...)]` attribute, merging into `out`. `seen`
/// tracks the keys set so far (across every `#[command]` attribute on the same
/// method) so a repeated key is rejected instead of silently overwriting.
pub fn parse_command_into(
    attr: &Attribute,
    out: &mut CommandAttr,
    seen: &mut SeenKeys,
) -> Result<(), Error> {
    let exprs = parse_attr_exprs(attr)?;
    for expr in exprs.iter() {
        match expr {
            Expr::Call(call) => {
                let func = func_ident(&call.func)?;
                if func == "annotations" {
                    seen.insert(&func)?;
                    out.annotations = Some(parse_annotations(call)?);
                } else {
                    return Err(Error::new(
                        call.func.span(),
                        format!("unknown #[command] entry `{func}(...)`"),
                    ));
                }
            }
            Expr::Assign(assign) => {
                let key = assign_left_ident(&assign.left)?;
                seen.insert(&key)?;
                match key.to_string().as_str() {
                    "aliases" => out.aliases = expr_str_array(&assign.right, "aliases")?,
                    "name" => out.name_override = Some(expr_str(&assign.right, "name")?),
                    "subtree" => out.subtree = Some(parse_subtree(&assign.right)?),
                    other => {
                        return Err(Error::new(
                            key.span(),
                            format!("unknown #[command] key `{other}`"),
                        ));
                    }
                }
            }
            other => {
                return Err(Error::new(
                    other.span(),
                    "expected `key = value` or `annotations(...)` in #[command(...)]",
                ));
            }
        }
    }

    // A `name` next to `subtree` overrides the grafted command name.
    if let (Some(name), Some(subtree)) = (&out.name_override, out.subtree.as_mut())
        && subtree.name_override.is_none()
    {
        subtree.name_override = Some(name.clone());
    }
    Ok(())
}

fn parse_subtree(right: &Expr) -> Result<SubtreeIr, Error> {
    match right {
        Expr::Path(p) => Ok(SubtreeIr {
            path: p.path.clone(),
            name_override: None,
        }),
        other => Err(Error::new(
            other.span(),
            "subtree must be a path to a #[tool_definition] trait, e.g. `subtree = path::Remote`",
        )),
    }
}

fn parse_annotations(call: &syn::ExprCall) -> Result<CommandAnnotationsIr, Error> {
    let mut ann = CommandAnnotationsIr::default();
    let mut seen = SeenKeys::default();
    for arg in call.args.iter() {
        // Accept both `key = bool` and a bare `key` (meaning `key = true`).
        let (key, value) = match arg {
            Expr::Assign(assign) => (
                assign_left_ident(&assign.left)?,
                expr_bool(&assign.right, "annotation")?,
            ),
            Expr::Path(p) => {
                let ident = p
                    .path
                    .get_ident()
                    .cloned()
                    .ok_or_else(|| Error::new(p.span(), "expected an annotation name"))?;
                (ident, true)
            }
            other => {
                return Err(Error::new(
                    other.span(),
                    "annotations entries must be `key` or `key = bool`",
                ));
            }
        };
        seen.insert(&key)?;
        match key.to_string().as_str() {
            "read_only" => ann.read_only = Some(value),
            "destructive" => ann.destructive = Some(value),
            "idempotent" => ann.idempotent = Some(value),
            "open_world" => ann.open_world = Some(value),
            other => {
                return Err(Error::new(
                    key.span(),
                    format!(
                        "unknown annotation `{other}`; expected: read_only, destructive, idempotent, open_world"
                    ),
                ));
            }
        }
    }
    Ok(ann)
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

    fn command(src: &str) -> Result<CommandAttr, Error> {
        let item: syn::ItemStruct = syn::parse_str(&format!("{src}\nstruct S;")).unwrap();
        let mut out = CommandAttr::default();
        let mut seen = SeenKeys::default();
        for attr in &item.attrs {
            parse_command_into(attr, &mut out, &mut seen)?;
        }
        Ok(out)
    }

    #[test]
    fn aliases_and_annotations() {
        let c =
            command(r#"#[command(aliases = ["ci"], annotations(destructive = true))]"#).unwrap();
        assert_eq!(c.aliases, vec!["ci".to_string()]);
        let ann = c.annotations.unwrap();
        assert_eq!(ann.destructive, Some(true));
        assert_eq!(ann.read_only, None);
    }

    #[test]
    fn multi_annotations() {
        let c = command(r#"#[command(annotations(read_only = true, idempotent = true))]"#).unwrap();
        let ann = c.annotations.unwrap();
        assert_eq!(ann.read_only, Some(true));
        assert_eq!(ann.idempotent, Some(true));
    }

    #[test]
    fn bare_annotation_is_true() {
        let c = command(r#"#[command(annotations(destructive, read_only = false))]"#).unwrap();
        let ann = c.annotations.unwrap();
        assert_eq!(ann.destructive, Some(true));
        assert_eq!(ann.read_only, Some(false));
    }

    #[test]
    fn subtree_path() {
        let c = command(r#"#[command(subtree = path::Remote)]"#).unwrap();
        let sub = c.subtree.unwrap();
        let path = &sub.path;
        assert_eq!(
            quote::quote!(#path).to_string().replace(' ', ""),
            "path::Remote"
        );
    }

    #[test]
    fn subtree_name_override() {
        let c = command(r#"#[command(subtree = path::Remote, name = "rmt")]"#).unwrap();
        let sub = c.subtree.unwrap();
        assert_eq!(sub.name_override.as_deref(), Some("rmt"));
    }

    #[test]
    fn unknown_key_is_error() {
        let err = command(r#"#[command(bogus = 1)]"#).unwrap_err();
        assert!(err.to_string().contains("unknown #[command] key"));
    }

    #[test]
    fn unknown_annotation_is_error() {
        let err = command(r#"#[command(annotations(whatever = true))]"#).unwrap_err();
        assert!(err.to_string().contains("unknown annotation"));
    }

    #[test]
    fn duplicate_annotation_key_is_error() {
        let err =
            command(r#"#[command(annotations(read_only = true, read_only = false))]"#).unwrap_err();
        assert!(err.to_string().contains("duplicate"));
    }

    #[test]
    fn duplicate_empty_aliases_key_is_error() {
        let err = command(r#"#[command(aliases = [], aliases = ["ci"])]"#).unwrap_err();
        assert!(err.to_string().contains("duplicate"));
    }
}
