use async_trait::async_trait;

use crate::context::Context;
use crate::preview2::wasi::keyvalue::atomic::{Bucket, Error, Host, Key};

#[async_trait]
impl Host for Context {
    async fn increment(
        &mut self,
        _bucket: Bucket,
        _key: Key,
        _delta: u64,
    ) -> anyhow::Result<Result<u64, Error>> {
        unimplemented!("increment")
    }

    async fn compare_and_swap(
        &mut self,
        _bucket: Bucket,
        _key: Key,
        _old: u64,
        _new: u64,
    ) -> anyhow::Result<Result<bool, Error>> {
        unimplemented!("compare_and_swap")
    }
}
