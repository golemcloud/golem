use lib::core;

fn main() {
    let (n, message) = core::hello(0);

    println!("{} And your luck number is {}.", message, n);
}
