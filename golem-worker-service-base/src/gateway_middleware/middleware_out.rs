pub trait MiddlewareOut<R> {
    async fn process_output(&self, output: R) -> R;
}