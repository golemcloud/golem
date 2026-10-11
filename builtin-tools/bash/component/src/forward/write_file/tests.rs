use super::*;

#[test]
fn text_within_the_size_limit_is_given_with_its_path() {
    assert_eq!(
        request(Path::new("/a/b.txt"), b"one\ntwo\n"),
        Some(("/a/b.txt", "one\ntwo\n"))
    );
    let most = vec![b'a'; MAX_BYTES];
    assert!(request(Path::new("/a/./b.txt"), &most).is_some());
}

/// The tool writes the bytes of a string as they are, so each character can go to it.
#[test]
fn text_with_a_nul_byte_or_a_long_character_is_given() {
    let text = "a\0b\r\n\u{feff}é😀";
    assert_eq!(
        request(Path::new("/a/b"), text.as_bytes()),
        Some(("/a/b", text))
    );
}

#[test]
fn content_above_the_size_limit_is_not_given() {
    let more = vec![b'a'; MAX_BYTES + 1];
    assert_eq!(request(Path::new("/a/b.txt"), &more), None);
}

#[test]
fn content_that_is_not_utf8_is_not_given() {
    assert_eq!(request(Path::new("/a/b.txt"), b"one\xff\n"), None);
}

#[test]
fn a_path_with_a_parent_component_is_not_given() {
    assert_eq!(request(Path::new("/a/../b.txt"), b"one\n"), None);
}

#[cfg(unix)]
#[test]
fn a_path_that_is_not_utf8_is_not_given() {
    use std::os::unix::ffi::OsStrExt;
    let path = Path::new(std::ffi::OsStr::from_bytes(b"/a/\xff.txt"));
    assert_eq!(request(path, b"one\n"), None);
}

#[test]
fn a_result_is_accepted_only_with_the_size_of_the_content() {
    let written = |bytes_written| Written {
        disposition: Disposition::Replaced,
        bytes_written,
    };
    assert_eq!(confirmed(&written(8), 8), Ok(()));
    assert!(confirmed(&written(7), 8).is_err());
    assert!(confirmed(&written(9), 8).is_err());
}

#[test]
fn only_the_definition_of_the_built_in_write_file_is_accepted() {
    crate::forward::tests::assert_accepts_only(include_str!("definition.json"), is_write_file);
}
