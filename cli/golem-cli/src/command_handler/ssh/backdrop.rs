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

//! The band behind each prompt of a session, which sets the session apart from the shell it
//! was started in. It has one colour on every terminal: the terminal is not asked what its
//! background is, and nothing about how the terminal looks is changed.

/// Set to `0`, it leaves the prompts without a band.
const VARIABLE: &str = "GOLEM_SSH_BACKGROUND";

/// The band's colour as SGR parameters, a dark shade with a hint of purple, for a terminal that
/// shows any colour.
const SHADE: &str = "48;2;48;45;53";

/// The nearest to it of the 256 colours, for a terminal that shows only those.
const NEAREST_SHADE: &str = "48;5;236";

/// The SGR parameters of the band, unless `GOLEM_SSH_BACKGROUND` says no. `var` reads an
/// environment variable.
pub fn band(var: impl Fn(&str) -> Option<String>) -> Option<&'static str> {
    if !wanted(&var) {
        return None;
    }
    let truecolor = var("COLORTERM").is_some_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "truecolor" | "24bit"
        )
    });
    Some(if truecolor { SHADE } else { NEAREST_SHADE })
}

fn wanted(var: &impl Fn(&str) -> Option<String>) -> bool {
    !var(VARIABLE).is_some_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "0" | "false" | "no" | "off"
        )
    })
}

#[cfg(test)]
mod tests {
    use super::{band, wanted};
    use test_r::test;

    #[test]
    fn the_band_has_one_colour_on_every_terminal() {
        let with = |set: &[(&str, &str)]| {
            let set: Vec<(String, String)> = set
                .iter()
                .map(|(name, value)| (name.to_string(), value.to_string()))
                .collect();
            band(move |name: &str| {
                set.iter()
                    .find(|(known, _)| known == name)
                    .map(|(_, value)| value.clone())
            })
        };
        // Exactly that colour where the terminal can show any colour,
        for any in ["truecolor", "24bit", " TrueColor "] {
            assert_eq!(
                with(&[("COLORTERM", any)]),
                Some("48;2;48;45;53"),
                "{any:?}"
            );
        }
        // and the nearest of its 256 where it cannot.
        assert_eq!(with(&[]), Some("48;5;236"));
        assert_eq!(with(&[("COLORTERM", "1")]), Some("48;5;236"));
        // The variable takes the band away.
        assert_eq!(
            with(&[("COLORTERM", "truecolor"), ("GOLEM_SSH_BACKGROUND", "0")]),
            None
        );
    }

    #[test]
    fn the_band_is_on_unless_the_variable_says_no() {
        let with = |value: Option<&str>| {
            let value = value.map(str::to_string);
            wanted(&move |name: &str| {
                (name == "GOLEM_SSH_BACKGROUND")
                    .then(|| value.clone())
                    .flatten()
            })
        };
        assert!(with(None));
        assert!(with(Some("1")));
        assert!(with(Some("")));
        for no in ["0", "false", "no", "off", " Off "] {
            assert!(!with(Some(no)), "{no:?}");
        }
    }
}
