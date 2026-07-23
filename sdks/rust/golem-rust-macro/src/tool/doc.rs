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

//! Parsing of `///` doc comments and `#[example(...)]` attributes into a
//! [`DocIr`] (summary + description + examples).

use crate::tool::helpers::{SeenKeys, expr_str, parse_attr_exprs};
use crate::tool::ir::{DocIr, ExampleIr};
use syn::spanned::Spanned;
use syn::{Attribute, Error, Expr, ExprLit, Lit, Meta};

/// Extracts the doc comment text from a list of attributes, splitting it into
/// the first paragraph (summary) and the remaining paragraphs (description).
/// Does not parse `#[example(...)]`; use [`parse_doc_full`] for that.
pub fn parse_doc(attrs: &[Attribute]) -> DocIr {
    let lines = collect_doc_lines(attrs);
    split_doc(&lines)
}

/// Like [`parse_doc`] but also collects `#[example(...)]` entries.
pub fn parse_doc_full(attrs: &[Attribute]) -> Result<DocIr, Error> {
    let mut doc = parse_doc(attrs);
    doc.examples = parse_examples(attrs)?;
    Ok(doc)
}

/// Collects `#[example(title = "...", body = "...")]` entries (repeatable).
fn parse_examples(attrs: &[Attribute]) -> Result<Vec<ExampleIr>, Error> {
    let mut out = Vec::new();
    for attr in attrs {
        if attr.path().is_ident("example") {
            out.push(parse_example_attr(attr)?);
        }
    }
    Ok(out)
}

fn parse_example_attr(attr: &Attribute) -> Result<ExampleIr, Error> {
    let exprs = parse_attr_exprs(attr)?;
    let mut title = String::new();
    let mut body = None;
    let mut seen = SeenKeys::default();
    for expr in exprs.iter() {
        let Expr::Assign(assign) = expr else {
            return Err(Error::new(
                expr.span(),
                "expected `title = \"...\"` / `body = \"...\"` in #[example(...)]",
            ));
        };
        let key = match &*assign.left {
            Expr::Path(p) if p.path.get_ident().is_some() => p.path.get_ident().unwrap().clone(),
            other => {
                return Err(Error::new(
                    other.span(),
                    "expected an identifier on the left of `=`",
                ));
            }
        };
        seen.insert(&key)?;
        match key.to_string().as_str() {
            "title" => title = expr_str(&assign.right, "title")?,
            "body" => body = Some(expr_str(&assign.right, "body")?),
            other => {
                return Err(Error::new(
                    assign.left.span(),
                    format!("unknown #[example] key `{other}`; expected `title` or `body`"),
                ));
            }
        }
    }
    let body = body.ok_or_else(|| Error::new(attr.span(), "#[example] is missing `body`"))?;
    Ok(ExampleIr { title, body })
}

/// Returns the raw doc lines (one per `#[doc = "..."]`), each with a single
/// leading space trimmed (rustdoc convention for `/// text`).
fn collect_doc_lines(attrs: &[Attribute]) -> Vec<String> {
    let mut lines = Vec::new();
    for attr in attrs {
        if !attr.path().is_ident("doc") {
            continue;
        }
        if let Meta::NameValue(nv) = &attr.meta
            && let Expr::Lit(ExprLit {
                lit: Lit::Str(s), ..
            }) = &nv.value
        {
            let value = s.value();
            lines.push(value.strip_prefix(' ').unwrap_or(&value).to_string());
        }
    }
    lines
}

fn split_doc(lines: &[String]) -> DocIr {
    // Drop leading/trailing blank lines.
    let trimmed: Vec<&String> = {
        let start = lines.iter().position(|l| !l.trim().is_empty());
        let end = lines.iter().rposition(|l| !l.trim().is_empty());
        match (start, end) {
            (Some(s), Some(e)) => lines[s..=e].iter().collect(),
            _ => Vec::new(),
        }
    };

    if trimmed.is_empty() {
        return DocIr::default();
    }

    // Summary is the first blank-line-delimited paragraph, joined into one line.
    let mut summary_lines = Vec::new();
    let mut idx = 0;
    while idx < trimmed.len() && !trimmed[idx].trim().is_empty() {
        summary_lines.push(trimmed[idx].trim());
        idx += 1;
    }
    let summary = summary_lines.join(" ").trim().to_string();

    // Skip blank separator lines before the description.
    while idx < trimmed.len() && trimmed[idx].trim().is_empty() {
        idx += 1;
    }
    let description = trimmed[idx..]
        .iter()
        .map(|l| l.as_str())
        .collect::<Vec<_>>()
        .join("\n")
        .trim_end()
        .to_string();

    DocIr {
        summary,
        description,
        examples: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc_attrs(src: &str) -> Vec<Attribute> {
        // Wrap the doc lines around a dummy item so we can parse real attributes.
        let item: syn::ItemFn = syn::parse_str(&format!("{src}\nfn f() {{}}")).unwrap();
        item.attrs
    }

    #[test]
    fn empty_doc() {
        let d = parse_doc(&[]);
        assert_eq!(d, DocIr::default());
    }

    #[test]
    fn summary_only() {
        let attrs = doc_attrs("/// Search files for a regex pattern.");
        let d = parse_doc(&attrs);
        assert_eq!(d.summary, "Search files for a regex pattern.");
        assert_eq!(d.description, "");
    }

    #[test]
    fn summary_and_description() {
        let attrs = doc_attrs(
            "/// Record changes to the repository.\n///\n/// The long form.\n/// Second line.",
        );
        let d = parse_doc(&attrs);
        assert_eq!(d.summary, "Record changes to the repository.");
        assert_eq!(d.description, "The long form.\nSecond line.");
    }

    #[test]
    fn examples_are_collected() {
        let item: syn::ItemFn = syn::parse_str(
            "/// Summary.\n#[example(title = \"basic\", body = \"grep foo\")]\n#[example(body = \"grep -i foo\")]\nfn f() {}",
        )
        .unwrap();
        let d = parse_doc_full(&item.attrs).unwrap();
        assert_eq!(d.summary, "Summary.");
        assert_eq!(d.examples.len(), 2);
        assert_eq!(d.examples[0].title, "basic");
        assert_eq!(d.examples[0].body, "grep foo");
        assert_eq!(d.examples[1].title, "");
        assert_eq!(d.examples[1].body, "grep -i foo");
    }

    #[test]
    fn example_without_body_is_error() {
        let item: syn::ItemFn = syn::parse_str("#[example(title = \"x\")]\nfn f() {}").unwrap();
        let err = parse_doc_full(&item.attrs).unwrap_err();
        assert!(err.to_string().contains("missing `body`"));
    }

    #[test]
    fn multiline_summary_is_joined() {
        let attrs = doc_attrs("/// Search files\n/// for a pattern.\n///\n/// Details here.");
        let d = parse_doc(&attrs);
        assert_eq!(d.summary, "Search files for a pattern.");
        assert_eq!(d.description, "Details here.");
    }
}
