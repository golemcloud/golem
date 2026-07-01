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

//! Parser for the `#[result(...)]` helper attribute.

use crate::tool::helpers::{SeenKeys, expr_str, expr_str_array, parse_attr_exprs};
use crate::tool::ir::ResultIr;
use syn::spanned::Spanned;
use syn::{Attribute, Error, Expr};

/// Parses a single `#[result(formatters = [...], default = "...")]` attribute.
pub fn parse_result(attr: &Attribute) -> Result<ResultIr, Error> {
    let exprs = parse_attr_exprs(attr)?;
    let mut ir = ResultIr::default();
    let mut seen = SeenKeys::default();
    for expr in exprs.iter() {
        let Expr::Assign(assign) = expr else {
            return Err(Error::new(
                expr.span(),
                "expected `formatters = [...]` or `default = \"...\"` in #[result(...)]",
            ));
        };
        let key = assign_left_ident(&assign.left)?;
        seen.insert(&key)?;
        match key.to_string().as_str() {
            "formatters" => ir.formatters = expr_str_array(&assign.right, "formatters")?,
            "default" => ir.default_formatter = Some(expr_str(&assign.right, "default")?),
            other => {
                return Err(Error::new(
                    key.span(),
                    format!("unknown #[result] key `{other}`"),
                ));
            }
        }
    }
    Ok(ir)
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

    fn result(src: &str) -> Result<ResultIr, Error> {
        let item: syn::ItemStruct = syn::parse_str(&format!("{src}\nstruct S;")).unwrap();
        parse_result(&item.attrs[0])
    }

    #[test]
    fn formatters_and_default() {
        let r =
            result(r#"#[result(formatters = ["human", "porcelain", "json"], default = "human")]"#)
                .unwrap();
        assert_eq!(r.formatters, vec!["human", "porcelain", "json"]);
        assert_eq!(r.default_formatter.as_deref(), Some("human"));
    }

    #[test]
    fn formatters_only() {
        let r = result(r#"#[result(formatters = ["oneline", "short"])]"#).unwrap();
        assert_eq!(r.formatters.len(), 2);
        assert_eq!(r.default_formatter, None);
    }

    #[test]
    fn unknown_key_is_error() {
        let err = result(r#"#[result(bogus = "x")]"#).unwrap_err();
        assert!(err.to_string().contains("unknown #[result] key"));
    }

    #[test]
    fn duplicate_result_key_is_error() {
        let err = result(r#"#[result(default = "human", default = "json")]"#).unwrap_err();
        assert!(err.to_string().contains("duplicate"));
    }
}
