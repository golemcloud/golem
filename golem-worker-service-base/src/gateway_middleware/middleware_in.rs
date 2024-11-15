trait MiddlewareIn<R> {
    async fn process_input(&self, input: R) -> R;
}
