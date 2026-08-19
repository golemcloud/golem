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

//! Terminal-friendly formatting of already-rendered agent ids: breaks long ids
//! across lines with structural indentation and optionally colors them.
//!
//! Done as a post-pass over the rendered text, re-tokenized with the shared
//! [`Lexer`]. Working on the rendered string (not the parsed value) keeps this
//! independent of the per-language renderers and of whatever is serialized;
//! since the lexer treats string literals as single tokens, brackets and commas
//! inside string values never affect the layout.

use super::lexer::{Lexer, Token};
use colored::Colorize;
use unicode_width::UnicodeWidthStr;

const INDENT: &str = "  ";
const MIN_WIDTH: usize = 20;

/// Lexical class of a token, used for both layout and coloring.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Open,
    Close,
    Comma,
    Punct,
    Str,
    Num,
    Lit,
    Ident,
}

/// A token as a byte range into the original rendered string.
struct Span {
    kind: Kind,
    start: usize,
    end: usize,
}

/// Formats an already-rendered agent id for terminal display.
///
/// `width` is what the caller can give the id (only the call site knows: a field
/// value is indented, a table cell bounded by its column); `None` disables line
/// breaking, leaving only coloring. Groups that fit stay inline; a single token
/// wider than `width` (a long string, a uuid) still overflows. Returned
/// unchanged if it cannot be tokenized.
pub fn format_agent_id_for_terminal(
    rendered: &str,
    colorize: bool,
    width: Option<usize>,
) -> String {
    let Some(spans) = tokenize(rendered) else {
        return rendered.to_string();
    };
    if spans.is_empty() {
        return rendered.to_string();
    }

    // Below the minimum every group explodes onto its own line, worse than
    // overflowing a very narrow target.
    let width = width
        .map(|width| width.max(MIN_WIDTH))
        .unwrap_or(usize::MAX);

    let close_of = match_brackets(&spans);
    layout(rendered, &spans, &close_of, colorize, width)
}

fn kind_of(token: &Token) -> Kind {
    match token {
        Token::LBrace | Token::LBrack | Token::LParen => Kind::Open,
        Token::RBrace | Token::RBrack | Token::RParen => Kind::Close,
        Token::Comma => Kind::Comma,
        Token::Colon | Token::DoubleColon | Token::Dot | Token::Eq | Token::Star | Token::At => {
            Kind::Punct
        }
        Token::StringLit(_) | Token::CharLit(_) => Kind::Str,
        Token::IntLit(_) | Token::UintLit(_) | Token::FloatLit(_) => Kind::Num,
        Token::BoolLit(_) | Token::Null | Token::Undefined => Kind::Lit,
        Token::Ident(_) => Kind::Ident,
        Token::Eof => Kind::Punct,
    }
}

/// Tokenizes the rendered id, recovering any character the lexer does not model
/// (map arrows `=>`, `<unknown>` fallbacks, the hyphens of a phantom uuid) as a
/// punctuation span instead of giving up — bailing out would drop both the line
/// breaking and the coloring for the whole id. `None` only if it cannot advance.
fn tokenize(input: &str) -> Option<Vec<Span>> {
    let mut spans: Vec<Span> = Vec::new();
    let mut base = 0usize;

    loop {
        let rest = &input[base..];
        let mut lexer = Lexer::new(rest);

        // Restarts after each recovered character.
        loop {
            match lexer.next_token() {
                Ok((Token::Eof, _, _)) => return Some(spans),
                Ok((token, start, end)) => spans.push(Span {
                    kind: kind_of(&token),
                    start: base + start,
                    end: base + end,
                }),
                Err(err) => {
                    let at = base + err.position;
                    let ch = input[at..].chars().next()?;

                    // Merge `=` + `>` into a single `=>` punctuation span.
                    let merged = ch == '>'
                        && spans.last().is_some_and(|last| {
                            last.end == at && &input[last.start..last.end] == "="
                        });
                    if merged {
                        spans.last_mut().expect("checked above").end = at + 1;
                    } else {
                        spans.push(Span {
                            kind: Kind::Punct,
                            start: at,
                            end: at + ch.len_utf8(),
                        });
                    }

                    base = at + ch.len_utf8();
                    break;
                }
            }
        }
    }
}

/// Maps each opening bracket span index to its matching closing span index.
fn match_brackets(spans: &[Span]) -> Vec<Option<usize>> {
    let mut close_of = vec![None; spans.len()];
    let mut open_stack = Vec::new();

    for (index, span) in spans.iter().enumerate() {
        match span.kind {
            Kind::Open => open_stack.push(index),
            Kind::Close => {
                if let Some(open) = open_stack.pop() {
                    close_of[open] = Some(index);
                }
            }
            _ => {}
        }
    }

    close_of
}

fn layout(
    input: &str,
    spans: &[Span],
    close_of: &[Option<usize>],
    colorize: bool,
    width: usize,
) -> String {
    let mut out = String::new();
    let mut col = 0usize;
    // One entry per open group: whether that group is broken across lines.
    let mut expanded: Vec<bool> = Vec::new();
    let mut prev_end: Option<usize> = None;
    let mut at_line_start = true;

    for (index, span) in spans.iter().enumerate() {
        let closing_expanded =
            span.kind == Kind::Close && expanded.last().copied().unwrap_or(false);

        // Keep the renderer's own spacing whenever we stay on the same line.
        if !at_line_start
            && !closing_expanded
            && let Some(end) = prev_end
        {
            let gap = &input[end..span.start];
            out.push_str(gap);
            col += gap.width();
        }
        at_line_start = false;

        let text = &input[span.start..span.end];

        match span.kind {
            Kind::Open => {
                let fits = close_of[index]
                    .map(|close| col + input[span.start..spans[close].end].width() <= width)
                    .unwrap_or(true);
                push_token(&mut out, &mut col, text, span.kind, colorize);
                expanded.push(!fits);

                if !fits {
                    newline_indent(&mut out, &mut col, indent_level(&expanded));
                    at_line_start = true;
                }
            }
            Kind::Close => {
                let was_expanded = expanded.pop().unwrap_or(false);
                if was_expanded {
                    newline_indent(&mut out, &mut col, indent_level(&expanded));
                }
                push_token(&mut out, &mut col, text, span.kind, colorize);
            }
            Kind::Comma => {
                push_token(&mut out, &mut col, text, span.kind, colorize);
                if expanded.last().copied().unwrap_or(false) {
                    newline_indent(&mut out, &mut col, indent_level(&expanded));
                    at_line_start = true;
                }
            }
            _ => push_token(&mut out, &mut col, text, span.kind, colorize),
        }

        prev_end = Some(span.end);
    }

    out
}

fn indent_level(expanded: &[bool]) -> usize {
    expanded.iter().filter(|is_expanded| **is_expanded).count()
}

fn newline_indent(out: &mut String, col: &mut usize, level: usize) {
    out.push('\n');
    out.push_str(&INDENT.repeat(level));
    *col = level * INDENT.width();
}

fn push_token(out: &mut String, col: &mut usize, text: &str, kind: Kind, colorize: bool) {
    if colorize {
        let colored = match kind {
            Kind::Str => text.green().to_string(),
            Kind::Num => text.cyan().to_string(),
            Kind::Lit => text.yellow().to_string(),
            Kind::Open | Kind::Close | Kind::Comma | Kind::Punct => text.dimmed().to_string(),
            Kind::Ident => text.to_string(),
        };
        out.push_str(&colored);
    } else {
        out.push_str(text);
    }
    *col += text.width();
}

#[cfg(test)]
mod tests {
    use super::format_agent_id_for_terminal;
    use test_r::test;

    #[test]
    fn short_id_stays_on_one_line() {
        let id = r#"Counter("main")"#;
        assert_eq!(format_agent_id_for_terminal(id, false, Some(80)), id);
    }

    #[test]
    fn plain_output_is_unchanged_when_it_fits() {
        let id = r#"Cart { user: "ann", items: [1, 2, 3] }"#;
        assert_eq!(format_agent_id_for_terminal(id, false, Some(80)), id);
    }

    #[test]
    fn long_id_is_broken_with_indentation() {
        let id = r#"ShoppingCart(user: "a-fairly-long-user-identifier", items: ["one", "two", "three"])"#;
        let formatted = format_agent_id_for_terminal(id, false, Some(40));

        // Fields go one per line, indented; the closing bracket returns to the
        // opener's level. The short inner list still fits, so it stays inline.
        assert_eq!(
            formatted,
            concat!(
                "ShoppingCart(\n",
                "  user: \"a-fairly-long-user-identifier\",\n",
                "  items: [\"one\", \"two\", \"three\"]\n",
                ")"
            )
        );
    }

    #[test]
    fn groups_that_still_fit_stay_inline() {
        let id = r#"Outer(first: "a-quite-long-value-here-indeed", inner: [1, 2])"#;
        let formatted = format_agent_id_for_terminal(id, false, Some(40));

        assert!(formatted.contains('\n'));
        // The short inner list is not exploded.
        assert!(
            formatted.contains("[1, 2]"),
            "inner list broken:\n{formatted}"
        );
    }

    #[test]
    fn braces_and_commas_inside_strings_do_not_affect_layout() {
        let id = r#"Weird(text: "a, b {c} [d]", n: 1)"#;
        let formatted = format_agent_id_for_terminal(id, false, Some(80));

        assert_eq!(formatted, id);
    }

    /// Each CJK character occupies two terminal columns. This id is 26 scalar values
    /// but 41 columns wide, so a scalar-count measure would wrongly keep it on one line
    /// in a 30-column terminal; display-width measurement must break it.
    #[test]
    fn wide_glyphs_are_measured_in_terminal_columns() {
        let id = "Counter(\"一二三四五六七八九十一二三四五\")";
        let formatted = format_agent_id_for_terminal(id, false, Some(30));

        assert!(
            formatted.contains('\n'),
            "wide-glyph id should break by column width, got:\n{formatted}"
        );
    }

    /// Combining marks add scalar values but no columns: twenty "e + combining acute"
    /// pairs are 40 scalars yet 20 columns. A scalar-count measure would wrongly break
    /// this in a 30-column terminal; by display width it fits and stays inline.
    #[test]
    fn combining_marks_do_not_inflate_measured_width() {
        let value = "e\u{0301}".repeat(20);
        let id = format!("Note(\"{value}\")");
        let formatted = format_agent_id_for_terminal(&id, false, Some(30));

        assert!(
            !formatted.contains('\n'),
            "combining-mark id fits by column width and should stay inline, got:\n{formatted}"
        );
    }

    #[test]
    fn untokenizable_input_is_returned_unchanged() {
        let id = "definitely ~not~ an agent id";
        assert_eq!(format_agent_id_for_terminal(id, false, Some(80)), id);
    }

    /// A phantom id ends in `[uuid]`, whose hyphens the lexer does not model.
    /// Recovering from them matters: bailing out would drop the layout and the
    /// coloring for the entire id, not just the suffix.
    #[test]
    fn phantom_uuid_suffix_is_still_formatted() {
        let id = r#"Probe("health-check-probe-with-a-longer-label", 3)[81db7b03-f3ff-456b-af38-a0fa0ce795b3]"#;
        let formatted = format_agent_id_for_terminal(id, false, Some(40));

        // The arguments break, the uuid is left intact on the closing line.
        assert_eq!(
            formatted,
            concat!(
                "Probe(\n",
                "  \"health-check-probe-with-a-longer-label\",\n",
                "  3\n",
                ")[81db7b03-f3ff-456b-af38-a0fa0ce795b3]"
            )
        );
    }

    /// A token wider than the target cannot be broken, but moving it onto its
    /// own indented line still recovers the width taken by the prefix.
    #[test]
    fn oversized_token_is_moved_to_its_own_line() {
        let id = r#"Probe("a-single-argument-that-is-really-quite-long")"#;
        let formatted = format_agent_id_for_terminal(id, false, Some(30));

        assert_eq!(
            formatted,
            concat!(
                "Probe(\n",
                "  \"a-single-argument-that-is-really-quite-long\"\n",
                ")"
            )
        );
    }

    #[test]
    fn no_width_never_breaks_lines() {
        let id = r#"ShoppingCart(user: "a-fairly-long-user-identifier", items: ["one", "two", "three"])"#;

        assert_eq!(format_agent_id_for_terminal(id, false, None), id);
    }

    #[test]
    fn map_arrows_are_tolerated() {
        let id = r#"Lookup({ "a" => 1, "b" => 2 })"#;
        // Must not panic and must preserve the arrows.
        let formatted = format_agent_id_for_terminal(id, false, Some(80));
        assert!(formatted.contains("=>"), "arrows lost: {formatted}");
    }
}
