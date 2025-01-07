pub use git_version::git_version;

macro_rules! lib_version {
    () => {{
        let version =
            crate::version::git_version!(args = ["--tags"], cargo_prefix = "", fallback = "1.0.0");
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

pub(crate) use lib_version;
