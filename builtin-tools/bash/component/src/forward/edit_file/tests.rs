use super::*;

/// The content after the tool has put `new_text` in the place of `old_text`, as `edit-file` does
/// it: `old_text` must start at one character of `content` and at no other.
fn applied(content: &str, old_text: &str, new_text: &str) -> Option<String> {
    let mut starts = content
        .char_indices()
        .filter(|(offset, _)| content[*offset..].starts_with(old_text));
    let (at, _) = starts.next()?;
    if old_text.is_empty() || starts.next().is_some() {
        return None;
    }
    Some([&content[..at], new_text, &content[at + old_text.len()..]].concat())
}

/// Asserts that the replacement for `old` and `new` is one that the tool accepts and that
/// gives `new`, and returns it.
fn checked<'a>(old: &'a str, new: &'a str) -> (&'a str, &'a str) {
    let (old_text, new_text) = replacement(old, new);
    assert_eq!(
        applied(old, old_text, new_text).as_deref(),
        Some(new),
        "{old:?} -> {new:?} by {old_text:?} -> {new_text:?}"
    );
    (old_text, new_text)
}

#[test]
fn a_change_inside_one_line_is_sent_without_the_rest_of_the_file() {
    assert_eq!(
        checked("one\ntwo\nthree\n", "one\n2\nthree\n"),
        ("two", "2")
    );
    // The two words end with the same letter, and that letter is not sent.
    assert_eq!(checked("alpha beta\n", "alpha gamma\n"), ("bet", "gamm"));
}

#[test]
fn a_change_of_several_lines_is_sent_from_the_first_change_to_the_last() {
    assert_eq!(
        checked("a1\nkeep\nb1\nend\n", "a2\nkeep\nb2\nend\n"),
        ("1\nkeep\nb1", "2\nkeep\nb2")
    );
}

#[test]
fn text_that_is_also_in_another_place_is_sent_with_its_line() {
    assert_eq!(checked("x=1\ny=1\n", "x=1\ny=2\n"), ("y=1\n", "y=2\n"));
}

#[test]
fn a_new_line_is_sent_with_the_lines_next_to_it() {
    assert_eq!(
        checked("1\n2\n3\n4\n5\n", "1\n2\nnew\n3\n4\n5\n"),
        ("2\n3\n", "2\nnew\n3\n")
    );
    assert_eq!(checked("1\n2\n3\n", "1\n2\n3\nnew\n"), ("3\n", "3\nnew\n"));
    assert_eq!(checked("1\n2\n3\n", "new\n1\n2\n3\n"), ("1\n", "new\n1\n"));
}

#[test]
fn a_removed_line_is_sent_alone() {
    assert_eq!(checked("a\nb\nc\n", "a\nc\n"), ("b\n", ""));
}

/// `sed -i` writes a file that it does not change, so the tool gets a replacement by the same
/// text.
#[test]
fn no_change_is_sent_as_the_last_line_by_itself() {
    assert_eq!(checked("one\ntwo\n", "one\ntwo\n"), ("two\n", "two\n"));
    assert_eq!(checked("one", "one"), ("one", "one"));
}

#[test]
fn text_that_no_line_makes_single_is_sent_as_the_whole_content() {
    let old = "a\n".repeat(20);
    let new = format!("{}b\n{}", "a\n".repeat(10), "a\n".repeat(9));
    assert_eq!(checked(&old, &new), (old.as_str(), new.as_str()));
    assert_eq!(checked("aa", "a"), ("aa", "a"));
}

/// The tool compares the text at each character of the content, up to the first byte that
/// differs. A long text that starts again and again costs it the product of the two lengths.
/// The whole content costs it one comparison. Here each line before the text is also a line of
/// the text, so a longer text starts again and again too.
#[test]
fn text_that_the_tool_finds_slowly_is_sent_as_the_whole_content() {
    let old = format!("{}end\n", "row\n".repeat(20_000));
    let new = "row\n".repeat(10_000);
    assert_eq!(checked(&old, &new), (old.as_str(), new.as_str()));
}

/// The line `x` is before the text that changes, and the content has an `x` in no other place.
/// A text that starts with that line costs the tool one byte in each other place.
#[test]
fn text_that_a_line_makes_fast_to_find_is_sent_from_that_line() {
    let rows = "row\n".repeat(20_000);
    let old = format!("{rows}x\n{}end\n", "row\n".repeat(10_000));
    let new = format!("{rows}x\n");
    let (old_text, new_text) = checked(&old, &new);
    assert!(old_text.len() < old.len(), "the whole content is sent");
    assert!(is_fast_to_find(&old, old_text));
    assert_eq!(new_text, "x\n");
}

#[test]
fn a_long_text_that_the_tool_finds_fast_is_sent_alone() {
    let block: String = (0..5_000).map(|line| format!("line {line}\n")).collect();
    let old = format!("head\n{block}tail\n");
    assert_eq!(checked(&old, "head\ntail\n"), (block.as_str(), ""));
}

#[test]
fn the_work_of_the_tool_is_counted_as_the_tool_does_it() {
    // One byte for each place where the text does not start, and the equal bytes where it does.
    assert!(is_fast_to_find("abcdefgh", "h"));
    assert!(is_fast_to_find("abcabcabd", "abd"));
    // 1,000 places, and 500 equal bytes at each of the first 500.
    let content = format!("{}b", "a".repeat(1_000));
    assert!(!is_fast_to_find(&content, &format!("{}b", "a".repeat(500))));
    assert!(is_fast_to_find(&content, &content));
    // The limit is four times the 1,001 bytes of the content, which is 4,004. The work is 3,993
    // for the first text and 4,986 for the second.
    assert!(is_fast_to_find(&content, "aaab"));
    assert!(!is_fast_to_find(&content, "aaaab"));
    // A long text costs one byte at each place where its first byte differs from the content.
    // That is 500 places here, and the text starts at one more place.
    let content = format!("{0}b{0}", "a".repeat(500));
    assert!(is_fast_to_find(&content, &format!("b{}", "a".repeat(500))));
}

#[test]
fn a_change_is_never_sent_from_inside_a_character() {
    // `é` and `è` have the same first byte, and `é` and `ĩ` have the same last byte.
    assert_eq!(checked("é\n", "è\n"), ("é", "è"));
    assert_eq!(checked("aé", "aĩ"), ("é", "ĩ"));
    assert_eq!(checked("日本語\n", "日本人\n"), ("語", "人"));
}

/// Many small contents, each changed in one place by a rule that depends only on its number.
#[test]
fn each_replacement_gives_the_new_content_and_is_one_that_the_tool_accepts() {
    let mut seed = 12_345_u32;
    let mut next = |limit: usize| {
        seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
        (seed >> 16) as usize % limit
    };
    let alphabet = ["a", "b", "\n", "é", "ab", "\n\n", " "];
    for _ in 0..3_000 {
        let mut text = |parts: usize| -> String {
            (0..parts).map(|_| alphabet[next(alphabet.len())]).collect()
        };
        let (head, middle, other, tail) = (text(6), text(3), text(3), text(6));
        let old = format!("{head}{middle}{tail}");
        if old.is_empty() {
            continue;
        }
        checked(&old, &format!("{head}{other}{tail}"));
    }
}

fn request_of<'a>(path: &'a str, old: &'a [u8], new: &'a [u8]) -> Option<Edit<'a>> {
    request(Path::new(path), old, new)
}

#[test]
fn a_change_of_text_is_given_with_its_path() {
    assert_eq!(
        request_of("/a/b.txt", b"one\ntwo\n", b"one\n2\n"),
        Some(Edit {
            path: "/a/b.txt",
            old_text: "two",
            new_text: "2",
        })
    );
}

#[test]
fn content_within_the_size_limit_is_given() {
    let most = "a".repeat(MAX_BYTES);
    assert!(request_of("/a/b", most.as_bytes(), b"b").is_some());
    assert!(request_of("/a/b", b"b", most.as_bytes()).is_some());
}

#[test]
fn content_above_the_size_limit_is_not_given() {
    let more = "a".repeat(MAX_BYTES + 1);
    assert_eq!(request_of("/a/b", more.as_bytes(), b"b"), None);
    assert_eq!(request_of("/a/b", b"b", more.as_bytes()), None);
}

/// The tool refuses an empty text to replace, and a file with no content has no other.
#[test]
fn a_file_with_no_content_is_not_given() {
    assert_eq!(request_of("/a/b", b"", b"new\n"), None);
    assert!(request_of("/a/b", b"old\n", b"").is_some());
}

/// The tool reads the file as text, and it refuses a file with a NUL byte.
#[test]
fn content_that_the_tool_does_not_read_as_text_is_not_given() {
    assert_eq!(request_of("/a/b", b"one\xff\n", b"1\n"), None);
    assert_eq!(request_of("/a/b", b"one\0\n", b"1\n"), None);
    assert_eq!(request_of("/a/b", b"one\n", b"1\xff\n"), None);
    assert!(request_of("/a/b", b"one\n", b"1\0\n").is_some());
}

#[test]
fn a_path_with_a_parent_component_is_not_given() {
    assert_eq!(request_of("/a/../b", b"one\n", b"1\n"), None);
}

#[cfg(unix)]
#[test]
fn a_path_that_is_not_utf8_is_not_given() {
    use std::os::unix::ffi::OsStrExt;
    let path = Path::new(std::ffi::OsStr::from_bytes(b"/a/\xff.txt"));
    assert_eq!(request(path, b"one\n", b"1\n"), None);
}

#[test]
fn a_result_is_accepted_only_for_one_replacement_with_both_sizes() {
    let edited = |replacements, bytes_before, bytes_after| Edited {
        replacements,
        bytes_before,
        bytes_after,
    };
    assert_eq!(confirmed(&edited(1, 8, 6), 8, 6), Ok(()));
    assert!(confirmed(&edited(2, 8, 6), 8, 6).is_err());
    assert!(confirmed(&edited(0, 8, 6), 8, 6).is_err());
    assert!(confirmed(&edited(1, 7, 6), 8, 6).is_err());
    assert!(confirmed(&edited(1, 8, 5), 8, 6).is_err());
}

#[test]
fn only_the_definition_of_the_built_in_edit_file_is_accepted() {
    crate::forward::tests::assert_accepts_only(include_str!("definition.json"), is_edit_file);
}
