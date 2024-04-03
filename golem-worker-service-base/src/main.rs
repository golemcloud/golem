use golem_worker_service_base::tokeniser::tokenizer::Tokenizer;

fn main() {
    let result = Tokenizer::new(">=").run();

    dbg!(result);
}