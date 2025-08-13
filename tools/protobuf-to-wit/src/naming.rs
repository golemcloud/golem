use once_cell::sync::Lazy;
use regex::Regex;

static CAMEL_BOUNDARY: Lazy<Regex> = Lazy::new(|| Regex::new(r"([a-z0-9])([A-Z])").unwrap());
static NON_ALNUM: Lazy<Regex> = Lazy::new(|| Regex::new(r"[^A-Za-z0-9]+").unwrap());

static RESERVED: &[&str] = &[
    "world", "interface", "use", "record", "type", "flag", "u8", "u16", "u32", "u64",
    "s8", "s16", "s32", "s64", "float32", "float64", "string", "bool", "list", "option",
    "result", "future", "stream", "tuple",
];

pub fn to_wit_ident(input: &str) -> String {
    let mut s = input.trim().to_string();
    if s.is_empty() { return "%unnamed".to_string(); }
    s = CAMEL_BOUNDARY.replace_all(&s, "$1-$2").to_string();
    s = NON_ALNUM.replace_all(&s, "-").to_string();
    s = s.to_lowercase();
    s = s.trim_matches('-').to_string();
    s = s.split('-').filter(|p| !p.is_empty()).collect::<Vec<_>>().join("-");
    let starts_alpha = s.chars().next().map(|c| c.is_ascii_alphabetic()).unwrap_or(false);
    if !starts_alpha || RESERVED.contains(&s.as_str()) {
        format!("%{}", s)
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::to_wit_ident;
    #[test]
    fn basics() {
        assert_eq!(to_wit_ident("TodoService"), "todo-service");
        assert_eq!(to_wit_ident("todo_id"), "todo-id");
        assert!(to_wit_ident("1bad").starts_with('%'));
    }
} 