//! `env`'s options that need more than a flag: `-S`'s split string, the signal options, and
//! GNU's quoting of the names `-v` and the diagnostics show.

/// A diagnostic `env` ends with, and its status.
pub(crate) type Failure = (String, u8);

/// A name as GNU quotes it in a message under a UTF-8 locale: `‘name’`.
pub(crate) fn quote(name: &str) -> String {
    format!("\u{2018}{name}\u{2019}")
}

/// `env -S STRING`'s arguments, split as GNU env splits them: blanks separate arguments; single
/// quotes keep everything but `\\` and `\'`; double quotes and bare text take the escapes `\c`
/// (ignore the rest), `\f`, `\n`, `\r`, `\t`, `\v`, `\#`, `\$`, `\_` (a blank: separating outside
/// quotes, a space inside), `\"`, `\'` and `\\`, and expand `${NAME}` from `env`; a `#` that
/// begins an argument begins a comment.
pub(crate) fn split_string(text: &str, env: &[(String, String)]) -> Result<Vec<String>, Failure> {
    let fail = |message: String| Err((format!("env: {message}\n"), 125));
    let mut words = Vec::new();
    let mut word = String::new();
    // Whether `word` is an argument yet: `''` is one, with no characters.
    let mut started = false;
    let mut quote: Option<char> = None;
    let mut chars = text.char_indices().peekable();
    while let Some((at, c)) = chars.next() {
        match (quote, c) {
            (Some('\''), '\'') | (Some('"'), '"') => quote = None,
            (Some('\''), '\\') if matches!(chars.peek(), Some((_, '\\' | '\''))) => {
                if let Some((_, next)) = chars.next() {
                    word.push(next);
                }
            }
            (Some('\''), other) => word.push(other),
            (_, '\\') => {
                let Some((_, escaped)) = chars.next() else {
                    return fail("invalid backslash at end of string in -S".to_owned());
                };
                match escaped {
                    'c' => break,
                    'f' => word.push('\x0c'),
                    'n' => word.push('\n'),
                    'r' => word.push('\r'),
                    't' => word.push('\t'),
                    'v' => word.push('\x0b'),
                    '#' | '$' | '"' | '\'' | '\\' => word.push(escaped),
                    '_' if quote.is_some() => word.push(' '),
                    '_' => {
                        if started {
                            words.push(std::mem::take(&mut word));
                            started = false;
                        }
                        continue;
                    }
                    other => return fail(format!("invalid sequence '\\{other}' in -S")),
                }
                started = true;
            }
            (_, '$') => {
                let rest = &text[at..];
                let Some(name) = rest
                    .strip_prefix("${")
                    .and_then(|braced| braced.split_once('}'))
                    .map(|(name, _)| name)
                    .filter(|name| is_name(name))
                else {
                    return fail(format!(
                        "only ${{VARNAME}} expansion is supported, error at: {rest}"
                    ));
                };
                for _ in 0..name.len() + 2 {
                    chars.next();
                }
                if let Some((_, value)) = env.iter().find(|(key, _)| key == name) {
                    word.push_str(value);
                }
                started = true;
            }
            (None, '\'' | '"') => {
                quote = Some(c);
                started = true;
            }
            (None, ' ' | '\t' | '\n' | '\x0b' | '\x0c' | '\r') => {
                if started {
                    words.push(std::mem::take(&mut word));
                    started = false;
                }
            }
            (None, '#') if !started => break,
            (_, other) => {
                word.push(other);
                started = true;
            }
        }
    }
    if quote.is_some() {
        return fail("no terminating quote in -S string".to_owned());
    }
    if started {
        words.push(word);
    }
    Ok(words)
}

fn is_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|first| first == '_' || first.is_ascii_alphabetic())
        && chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

/// The signals a Linux system with musl numbers, by name as `env --list-signal-handling` shows
/// them: 1 to 31, then the real-time signals the C library leaves to programs, 35 to 64.
pub(crate) fn signal_name(number: u8) -> Option<String> {
    const NAMES: [&str; 31] = [
        "HUP", "INT", "QUIT", "ILL", "TRAP", "ABRT", "BUS", "FPE", "KILL", "USR1", "SEGV", "USR2",
        "PIPE", "ALRM", "TERM", "STKFLT", "CHLD", "CONT", "STOP", "TSTP", "TTIN", "TTOU", "URG",
        "XCPU", "XFSZ", "VTALRM", "PROF", "WINCH", "POLL", "PWR", "SYS",
    ];
    const RTMIN: u8 = 35;
    const RTMAX: u8 = 64;
    match number {
        1..=31 => Some(NAMES[usize::from(number - 1)].to_owned()),
        RTMIN => Some("RTMIN".to_owned()),
        RTMAX => Some("RTMAX".to_owned()),
        n if (RTMIN..RTMAX).contains(&n) && n - RTMIN <= (RTMAX - RTMIN) / 2 => {
            Some(format!("RTMIN+{}", n - RTMIN))
        }
        n if (RTMIN..RTMAX).contains(&n) => Some(format!("RTMAX-{}", RTMAX - n)),
        _ => None,
    }
}

/// Every signal a program can ignore or block, in order: all but KILL and STOP.
pub(crate) fn catchable_signals() -> impl Iterator<Item = u8> {
    (1..=64).filter(|&n| n != 9 && n != 19 && signal_name(n).is_some())
}

/// A signal option's list: names with or without `SIG`, in any case, or numbers, separated by
/// commas; an empty item is skipped. Every catchable signal when there is no list.
pub(crate) fn signal_list(list: Option<&str>) -> Result<Vec<u8>, Failure> {
    let Some(list) = list else {
        return Ok(catchable_signals().collect());
    };
    let mut signals = Vec::new();
    for item in list.split(',').filter(|item| !item.is_empty()) {
        let number = item
            .parse::<u8>()
            .ok()
            .filter(|&n| signal_name(n).is_some())
            .or_else(|| {
                let upper = item.to_ascii_uppercase();
                let name = upper.strip_prefix("SIG").unwrap_or(&upper);
                (1..=64).find(|&n| signal_name(n).as_deref() == Some(name))
            });
        match number {
            Some(number) => signals.push(number),
            None => {
                return Err((
                    format!(
                        "env: {}: invalid signal\nTry 'env --help' for more information.\n",
                        quote(item)
                    ),
                    125,
                ));
            }
        }
    }
    Ok(signals)
}

/// How `env` leaves one signal for its command.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Handling {
    /// `Some(true)` to ignore it, `Some(false)` to restore its default action.
    pub(crate) ignore: Option<bool>,
    pub(crate) block: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn split(text: &str) -> Result<Vec<String>, Failure> {
        split_string(text, &[("X".to_owned(), "hi".to_owned())])
    }

    #[test]
    fn splits_as_gnu_env_does() {
        // Each checked against GNU env in the conformance oracle.
        assert_eq!(
            split("printf %s-%s\\n a b").unwrap(),
            ["printf", "%s-%s\n", "a", "b"]
        );
        assert_eq!(split("echo ${X}").unwrap(), ["echo", "hi"]);
        assert_eq!(
            split("printf '%s|' 'a b' c").unwrap(),
            ["printf", "%s|", "a b", "c"]
        );
        assert_eq!(
            split("echo a\\tb\\_c #comment").unwrap(),
            ["echo", "a\tb", "c"]
        );
        assert_eq!(
            split("echo a\\_b \"c\\_d\" \\c ignored").unwrap(),
            ["echo", "a", "b", "c d"]
        );
        assert_eq!(
            split("echo 'a\\'b' 'x\\\\y' \"q\\\"r\" \\t|").unwrap(),
            ["echo", "a'b", "x\\y", "q\"r", "\t|"]
        );
        assert_eq!(split("echo a#b '' x").unwrap(), ["echo", "a#b", "", "x"]);
        assert_eq!(split("").unwrap(), Vec::<String>::new());
        assert_eq!(
            split("echo $X").unwrap_err().0,
            "env: only ${VARNAME} expansion is supported, error at: $X\n"
        );
        assert_eq!(
            split("echo \"unterminated").unwrap_err().0,
            "env: no terminating quote in -S string\n"
        );
    }

    #[test]
    fn names_signals_as_musl_numbers_them() {
        assert_eq!(signal_list(Some("INT,sigterm,2,")).unwrap(), [2, 15, 2]);
        assert_eq!(catchable_signals().count(), 59);
        assert_eq!(signal_name(36).as_deref(), Some("RTMIN+1"));
        assert_eq!(signal_name(63).as_deref(), Some("RTMAX-1"));
        assert_eq!(
            signal_list(Some("NOPE")).unwrap_err().0,
            "env: \u{2018}NOPE\u{2019}: invalid signal\nTry 'env --help' for more information.\n"
        );
    }
}
