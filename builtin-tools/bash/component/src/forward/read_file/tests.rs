use super::*;

fn page(content: &str, next: Option<u64>) -> Result<Page, Refusal> {
    Ok(Page {
        content: content.into(),
        start_line: None,
        end_line: None,
        next_cursor: next.map(|byte_offset| Cursor {
            byte_offset,
            line: 1,
        }),
    })
}

/// The byte offset of the cursor that each call sent, and `None` for a call with no cursor.
type Sent = Vec<Option<u64>>;

/// Reads a file of `len` bytes from `pages`, one page for each call. Returns the result and
/// what each call sent.
fn read(len: u64, pages: Vec<Result<Page, Refusal>>) -> (Result<Option<Vec<u8>>, String>, Sent) {
    let mut sent = Vec::new();
    let mut pages = pages.into_iter();
    let result = read_all(len, |cursor| {
        sent.push(cursor.map(|cursor| cursor.byte_offset));
        pages.next().expect("a call after the last page")
    });
    (result, sent)
}

#[test]
fn a_file_within_the_size_limit_is_asked_for_by_its_path() {
    assert_eq!(request(Path::new("/a/b.txt"), 10), Some("/a/b.txt"));
    assert_eq!(
        request(Path::new("/a/./b.txt"), MAX_BYTES),
        Some("/a/./b.txt")
    );
}

#[test]
fn a_file_above_the_size_limit_is_not_asked_for() {
    assert_eq!(request(Path::new("/a/b.txt"), MAX_BYTES + 1), None);
}

#[test]
fn a_path_with_a_parent_component_is_not_asked_for() {
    assert_eq!(request(Path::new("/a/../b.txt"), 10), None);
}

#[cfg(unix)]
#[test]
fn a_path_that_is_not_utf8_is_not_asked_for() {
    use std::os::unix::ffi::OsStrExt;
    let path = Path::new(std::ffi::OsStr::from_bytes(b"/a/\xff.txt"));
    assert_eq!(request(path, 10), None);
}

#[test]
fn one_page_is_the_whole_file() {
    let (result, sent) = read(4, vec![page("ab\nc", None)]);
    assert_eq!(result, Ok(Some(b"ab\nc".to_vec())));
    assert_eq!(sent, [None]);
}

#[test]
fn pages_are_joined_in_order_and_each_cursor_goes_back() {
    let (result, sent) = read(
        6,
        vec![page("ab", Some(2)), page("cd", Some(4)), page("ef", None)],
    );
    assert_eq!(result, Ok(Some(b"abcdef".to_vec())));
    assert_eq!(sent, [None, Some(2), Some(4)]);
}

#[test]
fn an_empty_last_page_ends_the_file() {
    let (result, _) = read(2, vec![page("ab", Some(2)), page("", None)]);
    assert_eq!(result, Ok(Some(b"ab".to_vec())));
}

#[test]
fn a_file_that_is_not_text_is_left_to_the_command() {
    let (result, _) = read(4, vec![Err(Refusal::NotText)]);
    assert_eq!(result, Ok(None));
    let (result, _) = read(4, vec![page("ab", Some(2)), Err(Refusal::NotText)]);
    assert_eq!(result, Ok(None));
}

#[test]
fn another_refusal_is_a_failure_of_the_tool() {
    let (result, _) = read(4, vec![Err(Refusal::Other("denied".into()))]);
    assert_eq!(result, Err("denied".into()));
}

#[test]
fn fewer_bytes_than_the_file_has_is_a_failure_of_the_tool() {
    let (result, _) = read(5, vec![page("ab", None)]);
    assert!(result.is_err(), "{result:?}");
}

#[test]
fn more_bytes_than_the_file_has_is_a_failure_of_the_tool() {
    let (result, sent) = read(1, vec![page("ab", Some(2))]);
    assert!(result.is_err(), "{result:?}");
    assert_eq!(sent.len(), 1);
}

#[test]
fn a_cursor_that_is_not_at_the_end_of_the_bytes_so_far_is_a_failure_of_the_tool() {
    let (result, sent) = read(6, vec![page("ab", Some(5))]);
    assert!(result.is_err(), "{result:?}");
    assert_eq!(sent.len(), 1);
}

#[test]
fn a_page_that_brings_nothing_before_the_end_is_a_failure_of_the_tool() {
    let (result, sent) = read(6, vec![page("ab", Some(2)), page("", Some(2))]);
    assert!(result.is_err(), "{result:?}");
    assert_eq!(sent.len(), 2);
}

/// The first page tells how many calls the file needs. A file that cannot fit in the limit of
/// calls costs one call, not all of them.
#[test]
fn a_file_that_needs_too_many_calls_is_left_to_the_command_after_one_call() {
    let (result, sent) = read(3 * MAX_CALLS + 1, vec![page("abc", Some(3))]);
    assert_eq!(result, Ok(None));
    assert_eq!(sent.len(), 1);
}

#[test]
fn a_file_is_read_with_the_limit_of_calls_and_not_with_one_call_more() {
    let pages = |count: u64| {
        (1..=count)
            .map(|call| page("a", (call < count).then_some(call)))
            .collect::<Vec<_>>()
    };
    let limit = usize::try_from(MAX_CALLS).unwrap();
    let (result, sent) = read(MAX_CALLS, pages(MAX_CALLS));
    assert_eq!(
        result.map(|bytes| bytes.map(|bytes| bytes.len())),
        Ok(Some(limit))
    );
    assert_eq!(sent.len(), limit);
    // The first page is long, so the estimate says that the file fits; the later pages are short.
    let mut slow = vec![page("0123456789", Some(10))];
    slow.extend((1..=MAX_CALLS).map(|call| page("a", Some(10 + call))));
    let (result, sent) = read(10 + MAX_CALLS + 1, slow);
    assert_eq!(result, Ok(None));
    assert_eq!(sent.len(), limit);
}

/// The tool has two errors for a file that is not UTF-8 text. It gives `invalid-range` for a
/// page whose first byte is a UTF-8 continuation byte, which no page of a text file starts with.
#[test]
fn an_error_of_the_tool_for_a_file_that_is_not_text_is_about_that_file_only() {
    assert_eq!(refusal(Some("binary-file"), "x".into()), Refusal::NotText);
    assert_eq!(refusal(Some("invalid-range"), "x".into()), Refusal::NotText);
    assert_eq!(
        refusal(Some("path-policy-denied"), "x".into()),
        Refusal::Other("x".into())
    );
    assert_eq!(refusal(None, "x".into()), Refusal::Other("x".into()));
}

/// For 64 KiB of text in fewer than 200 lines and then a continuation byte, the first page is
/// the text, and the next page starts with the continuation byte.
#[test]
fn a_later_page_that_starts_with_a_continuation_byte_leaves_the_file_to_the_command() {
    let not_text = refusal(Some("invalid-range"), "x".into());
    let (result, sent) = read(5, vec![page("ab", Some(2)), Err(not_text)]);
    assert_eq!(result, Ok(None));
    assert_eq!(sent, [None, Some(2)]);
}

#[test]
fn only_the_definition_of_the_built_in_read_file_is_accepted() {
    crate::forward::tests::assert_accepts_only(include_str!("definition.json"), is_read_file);
}
