#[cfg(test)]
test_r::enable!();

#[cfg(test)]
mod examples {
    use assert2::{assert, let_assert};
    use std::process::Command;
    use test_r::test;

    #[test]
    fn examples_c() {
        test_examples("c")
    }

    #[test]
    fn examples_go() {
        test_examples("go")
    }

    #[test]
    fn examples_js() {
        test_examples("js")
    }

    #[test]
    fn examples_python() {
        test_examples("python")
    }

    #[test]
    fn examples_rust() {
        test_examples("rust")
    }

    #[test]
    fn examples_ts() {
        test_examples("ts")
    }

    #[test]
    fn examples_zig() {
        test_examples("zig")
    }

    fn test_examples(test_prefix: &str) {
        let status = Command::new("../target/debug/golem-examples-test-cli")
            .args([
                "examples",
                "--filter",
                &format!("^{}-", test_prefix),
                "--target-path",
                "../target/examples-test",
            ])
            .status();
        let_assert!(Ok(status) = status);
        assert!(status.success());
    }
}

#[cfg(test)]
mod app {
    use assert2::{assert, let_assert};
    use std::process::Command;
    use test_r::test;

    #[test]
    fn app_with_all_lang() {
        let status = Command::new("../target/debug/golem-examples-test-cli")
            .args(["app"])
            .status();
        let_assert!(Ok(status) = status);
        assert!(status.success());
    }
}
