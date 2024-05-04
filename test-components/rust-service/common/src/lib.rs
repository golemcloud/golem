use std::collections::HashMap;

pub fn echo(input: String) -> String {
    format!("echo: {input}")
}

pub fn fibonacci(n: u64) -> u64 {
    let mut memo: HashMap<u64, u64> = HashMap::new();
    fn fib_helper(n: u64, memo: &mut HashMap<u64, u64>) -> u64 {
        if let Some(result) = memo.get(&n) {
            return *result;
        }
        let value = if n <= 1 {
            n
        } else {
            fib_helper(n - 1, memo) + fib_helper(n - 2, memo)
        };
        memo.insert(n, value);
        value
    }
    fib_helper(n, &mut memo)
}

pub fn loop_fibonacci(num: u64, loops: u64) {
    for _in in 0..loops {
        let _ = fibonacci(num);
    }
}

pub fn factorial(num: u128) -> u128 {
    (1..=num).product()
}

pub fn loop_factorial(num: u128, loops: u64) {
    for _in in 0..loops {
        let _ = factorial(num);
    }
}

#[cfg(test)]
mod tests {
    use crate::fibonacci;

    #[test]
    pub fn fibonacci_test() {
        let v = fibonacci(50);

        println!("{v}");
    }
}
