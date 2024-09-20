pub use git_version::git_version;

#[macro_export]
macro_rules! golem_version {
    () => {{
        let version = golem_common::golem_version::git_version!(
            args = ["--tags", "--long"],
            cargo_prefix = "",
            fallback = "0.0.0"
        );
        if !version.is_empty() && version.as_bytes()[0] == b'v' {
            unsafe {
                std::str::from_utf8_unchecked(std::slice::from_raw_parts(
                    version.as_ptr().add(1),
                    version.len() - 1,
                ))
            }
        } else {
            version
        }
    }};
}
