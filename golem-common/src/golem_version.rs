pub use git_version::git_version;

#[macro_export]
macro_rules! golem_version {
    () => {
        golem_common::golem_version::git_version!(
            args = ["--tags", "--long", "--dirty"],
            cargo_prefix = "",
            fallback = "0.0.0"
        )
    };
}
