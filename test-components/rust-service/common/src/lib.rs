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

pub fn loop_fibonacci(num: u64, loops: u64) -> (u64, u64) {
    let mut iterations: u64 = 0;
    let mut value: u64 = 0;
    for _in in 0..loops {
        value = fibonacci(num);
        iterations += 1;
    }
    (iterations, value)
}

pub fn calculate_sum(num: u64, loops: u64) -> (u64, u128) {
    fn process_vec(vec: Vec<u64>) -> u64 {
        vec.iter().sum()
    }
    let vec = (0..num).collect::<Vec<u64>>();
    let mut iterations: u64 = 0;
    let mut value: u128 = 0;
    for i in 0..loops {
        let mut vec = vec.clone();
        if i % 2 == 0 {
            vec.reverse();
        }
        value += process_vec(vec) as u128;
        iterations += 1;
    }
    (iterations, value)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommonData {
    pub id: String,
    pub name: String,
    pub desc: String,
    pub timestamp: u64,
}

pub fn process_data(data: Vec<CommonData>) -> Vec<CommonData> {
    let mut vec = data.clone();
    vec.reverse();
    vec.into_iter()
        .map(|d| CommonData {
            id: d.id.chars().rev().collect(),
            name: d.name.chars().rev().collect(),
            desc: d.desc.chars().rev().collect(),
            timestamp: d.timestamp,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use crate::fibonacci;
    use crate::calculate_sum;
    use std::time::SystemTime;

    #[test]
    pub fn fibonacci_test() {
        let v = fibonacci(50);

        println!("{v}");
    }

    #[test]
    pub fn calculate_sum_test() {
        let start = SystemTime::now();
        let v = calculate_sum(10000, 200000);
        let elapsed = start.elapsed().expect("SystemTime elapsed failed");

        println!("{:?} {:?}", v, elapsed);
    }
}
