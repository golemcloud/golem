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

use super::{GITHUB_ISSUER, Pair, PrincipalRef};
use serde_json::Value;
use std::str::FromStr;
use test_r::test;

fn text_form_vectors() -> Value {
    serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../test-data/caller-identity/text-form-v1.json"
    )))
    .unwrap()
}

fn cases<'a>(vectors: &'a Value, section: &str) -> &'a Vec<Value> {
    let cases = vectors[section].as_array().unwrap();
    assert!(!cases.is_empty(), "no `{section}` cases");
    cases
}

#[test]
fn text_form_prints_and_parses_every_golden_pair() {
    let vectors = text_form_vectors();
    assert_eq!(vectors["version"], 1);
    for case in cases(&vectors, "roundTrip") {
        let name = case["name"].as_str().unwrap();
        let pair = Pair::new(
            case["issuer"].as_str().unwrap(),
            case["subject"].as_str().unwrap(),
        );
        let text = case["text"].as_str().unwrap();

        assert_eq!(pair.to_string(), text, "printing `{name}`");
        assert_eq!(Pair::from_str(text), Ok(pair.clone()), "parsing `{name}`");

        let reference = PrincipalRef::new(pair);
        assert_eq!(
            reference.to_string(),
            text,
            "printing the reference `{name}`"
        );
        assert_eq!(
            PrincipalRef::from_str(text),
            Ok(reference),
            "parsing the reference `{name}`"
        );
    }
}

#[test]
fn text_form_escapes_every_control_character_in_one_way() {
    for code in 0u32..0x20 {
        let character = char::from_u32(code).unwrap();
        let expected_escape = match character {
            '\u{08}' => "\\b".to_string(),
            '\t' => "\\t".to_string(),
            '\n' => "\\n".to_string(),
            '\u{0c}' => "\\f".to_string(),
            '\r' => "\\r".to_string(),
            _ => format!("\\u{code:04x}"),
        };
        let pair = Pair::new(format!("i{character}"), format!("{character}s"));
        let text = pair.to_string();

        assert_eq!(
            text,
            format!("(\"i{expected_escape}\",\"{expected_escape}s\")"),
            "printing U+{code:04X}"
        );
        assert_eq!(Pair::from_str(&text), Ok(pair), "parsing U+{code:04X}");
    }
}

#[test]
fn text_form_writes_slash_and_non_ascii_text_unescaped() {
    let pair = Pair::new(
        "https://login.acme.example/",
        "\u{e9}/\u{1f600}\u{2028}\u{7f}",
    );
    let text = pair.to_string();

    assert_eq!(
        text,
        "(\"https://login.acme.example/\",\"\u{e9}/\u{1f600}\u{2028}\u{7f}\")"
    );
    assert!(!text.contains("\\/"));
    assert!(!text.contains("\\u"));
}

#[test]
fn text_form_parser_refuses_every_golden_non_canonical_text() {
    let vectors = text_form_vectors();
    for case in cases(&vectors, "reject") {
        let name = case["name"].as_str().unwrap();
        let text = case["text"].as_str().unwrap();
        assert!(
            Pair::from_str(text).is_err(),
            "`{name}` was accepted: {text}"
        );
        assert!(
            PrincipalRef::from_str(text).is_err(),
            "`{name}` was accepted as a reference: {text}"
        );
    }
}

#[test]
fn text_form_parser_accepts_a_unicode_escape_only_where_the_printer_writes_one() {
    let short_escapes = [0x08u32, 0x09, 0x0a, 0x0c, 0x0d];
    for code in 0u32..0x100 {
        let lowercase = format!("(\"i\",\"\\u{code:04x}\")");
        let uppercase = format!("(\"i\",\"\\u{code:04X}\")");
        let printed = code < 0x20 && !short_escapes.contains(&code);

        match Pair::from_str(&lowercase) {
            Ok(pair) => {
                assert!(printed, "accepted the non-canonical {lowercase}");
                assert_eq!(pair.subject, char::from_u32(code).unwrap().to_string());
            }
            Err(_) => assert!(!printed, "refused the canonical {lowercase}"),
        }
        if uppercase != lowercase {
            assert!(
                Pair::from_str(&uppercase).is_err(),
                "accepted uppercase hex {uppercase}"
            );
        }
    }
}

#[test]
fn github_user_pair_is_the_github_issuer_and_the_numeric_user_id() {
    assert_eq!(GITHUB_ISSUER, "https://github.com");
    assert_eq!(
        Pair::github_user(583231),
        Pair::new("https://github.com", "583231")
    );
}
