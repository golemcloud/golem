use async_trait::async_trait;

use crate::context::Context;
use crate::preview2::wasi::keyvalue::cache::{
    Error, FutureExistsResult, FutureGetOrSetResult, FutureGetResult, FutureResult, GetOrSetEntry,
    Host, IncomingValue, Key, OutgoingValue, Pollable, Vacancy,
};

#[async_trait]
impl Host for Context {
    async fn get(&mut self, _k: Key) -> anyhow::Result<FutureGetResult> {
        unimplemented!("get")
    }

    async fn drop_future_get_result(&mut self, _f: FutureGetResult) -> anyhow::Result<()> {
        unimplemented!("drop_future_get_result")
    }

    async fn future_get_result_get(
        &mut self,
        _f: FutureGetResult,
    ) -> anyhow::Result<Option<Result<Option<IncomingValue>, Error>>> {
        unimplemented!("future_get_result_get")
    }

    async fn listen_to_future_get_result(
        &mut self,
        _f: FutureGetResult,
    ) -> anyhow::Result<Pollable> {
        unimplemented!("listen_to_future_get_result")
    }

    async fn exists(&mut self, _k: Key) -> anyhow::Result<FutureExistsResult> {
        unimplemented!("exists")
    }

    async fn drop_future_exists_result(&mut self, _f: FutureExistsResult) -> anyhow::Result<()> {
        unimplemented!("drop_future_exists_result")
    }

    async fn future_exists_result_get(
        &mut self,
        _f: FutureExistsResult,
    ) -> anyhow::Result<Option<Result<bool, Error>>> {
        unimplemented!("future_exists_result_get")
    }

    async fn listen_to_future_exists_result(
        &mut self,
        _f: FutureExistsResult,
    ) -> anyhow::Result<Pollable> {
        unimplemented!("listen_to_future_exists_result")
    }

    async fn set(
        &mut self,
        _k: Key,
        _v: OutgoingValue,
        _ttl_ms: Option<u32>,
    ) -> anyhow::Result<FutureResult> {
        unimplemented!("set")
    }

    async fn drop_future_result(&mut self, _f: FutureResult) -> anyhow::Result<()> {
        unimplemented!("drop_future_result")
    }

    async fn future_result_get(
        &mut self,
        _f: FutureResult,
    ) -> anyhow::Result<Option<Result<(), Error>>> {
        unimplemented!("future_result_get")
    }

    async fn listen_to_future_result(&mut self, _f: FutureResult) -> anyhow::Result<Pollable> {
        unimplemented!("listen_to_future_result")
    }

    async fn get_or_set(&mut self, _k: Key) -> anyhow::Result<FutureGetOrSetResult> {
        unimplemented!("get_or_set")
    }

    async fn drop_future_get_or_set_result(
        &mut self,
        _f: FutureGetOrSetResult,
    ) -> anyhow::Result<()> {
        unimplemented!("drop_future_get_or_set_result")
    }

    async fn future_get_or_set_result_get(
        &mut self,
        _f: FutureGetOrSetResult,
    ) -> anyhow::Result<Option<Result<GetOrSetEntry, Error>>> {
        unimplemented!("future_get_or_set_result_get")
    }

    async fn listen_to_future_get_or_set_result(
        &mut self,
        _f: FutureGetOrSetResult,
    ) -> anyhow::Result<Pollable> {
        unimplemented!("listen_to_future_get_or_set_result")
    }

    async fn drop_vacancy(&mut self, _v: Vacancy) -> anyhow::Result<()> {
        unimplemented!("drop_vacancy")
    }

    async fn vacancy_fill(
        &mut self,
        _v: Vacancy,
        _ttl_ms: Option<u32>,
    ) -> anyhow::Result<OutgoingValue> {
        unimplemented!("vacancy_fill")
    }

    async fn delete(&mut self, _k: Key) -> anyhow::Result<FutureResult> {
        unimplemented!("delete")
    }
}
