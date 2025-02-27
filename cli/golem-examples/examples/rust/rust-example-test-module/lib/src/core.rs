pub fn hello(n: usize) -> (usize, String) {
    let message = match n {
        0 => "Hello there! My name is Golem. Nice to meet you!".to_string(),
        1 => "Hello there! Welcome back to Golem Cloud!".to_string(),
        _ => format!(
            "Hello there! In the world of Golem Cloud, {} times is a charm! Have a great day!",
            n
        ),
    };

    (n + 1, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hello_tests_should_return_expected_count_and_message() {
        assert_eq!(
            hello(0),
            (
                1,
                "Hello there! My name is Golem. Nice to meet you!".to_string()
            )
        );
        assert_eq!(
            hello(1),
            (2, "Hello there! Welcome back to Golem Cloud!".to_string())
        );
        assert_eq!(
            hello(3),
            (
                4,
                "Hello there! In the world of Golem Cloud, 3 times is a charm! Have a great day!"
                    .to_string()
            )
        );
    }
}
