use async_trait::async_trait;
use wasmtime::component::Resource;

use crate::context::Context;
use crate::preview2::wasi::keyvalue::cache::{Error, FutureExistsResult, FutureGetOrSetResult, FutureGetResult, FutureResult, GetOrSetEntry, Host, HostFutureExistsResult, HostFutureGetOrSetResult, HostFutureGetResult, HostFutureResult, HostVacancy, IncomingValue, Key, OutgoingValue, Pollable, Vacancy};

#[async_trait]
impl HostVacancy for Context {
    async fn vacancy_fill(&mut self, _self_: Resource<Vacancy>, _ttl_ms: Option<u32>) -> anyhow::Result<Resource<OutgoingValue>> {
        unimplemented!("vacancy_fill")
    }

    fn drop(&mut self, _rep: Resource<Vacancy>) -> anyhow::Result<()> {
        unimplemented!()
    }
}

#[async_trait]
impl HostFutureGetOrSetResult for Context {
    async fn future_get_or_set_result_get(&mut self, _self_: Resource<FutureGetOrSetResult>) -> anyhow::Result<Option<Result<GetOrSetEntry, Resource<Error>>>> {
        unimplemented!("future_get_or_set_result_get")
    }

    async fn listen_to_future_get_or_set_result(&mut self, _self_: Resource<FutureGetOrSetResult>) -> anyhow::Result<Resource<Pollable>> {
        unimplemented!("listen_to_future_get_or_set_result")
    }

    fn drop(&mut self, _rep: Resource<FutureGetOrSetResult>) -> anyhow::Result<()> {
        unimplemented!()
    }
}

#[async_trait]
impl HostFutureResult for Context {
    async fn future_result_get(&mut self, _self_: Resource<FutureResult>) -> anyhow::Result<Option<Result<(), Resource<Error>>>> {
        unimplemented!("future_result_get")
    }

    async fn listen_to_future_result(&mut self, _self_: Resource<FutureResult>) -> anyhow::Result<Resource<Pollable>> {
        unimplemented!("listen_to_future_result")
    }

    fn drop(&mut self, _rep: Resource<FutureResult>) -> anyhow::Result<()> {
        unimplemented!()
    }
}

#[async_trait]
impl HostFutureExistsResult for Context {
    async fn future_exists_result_get(&mut self, _self_: Resource<FutureExistsResult>) -> anyhow::Result<Option<Result<bool, Resource<Error>>>> {
        unimplemented!("future_exists_result_get")
    }

    async fn listen_to_future_exists_result(&mut self, _self_: Resource<FutureExistsResult>) -> anyhow::Result<Resource<Pollable>> {
        unimplemented!("listen_to_future_exists_result")
    }

    fn drop(&mut self, _rep: Resource<FutureExistsResult>) -> anyhow::Result<()> {
        unimplemented!()
    }
}

#[async_trait]
impl HostFutureGetResult for Context {
    async fn future_get_result_get(&mut self, _self_: Resource<FutureGetResult>) -> anyhow::Result<Option<Result<Option<Resource<IncomingValue>>, Resource<Error>>>> {
        unimplemented!("future_get_result_get")
    }

    async fn listen_to_future_get_result(&mut self, _self_: Resource<FutureGetResult>) -> anyhow::Result<Resource<Pollable>> {
        unimplemented!("listen_to_future_get_result")
    }

    fn drop(&mut self, _rep: Resource<FutureGetResult>) -> anyhow::Result<()> {
        unimplemented!()
    }
}

#[async_trait]
impl Host for Context {
    async fn get(&mut self, _k: Key) -> anyhow::Result<Resource<FutureGetResult>> {
        unimplemented!("get")
    }

    async fn exists(&mut self, _k: Key) -> anyhow::Result<Resource<FutureExistsResult>> {
        unimplemented!("exists")
    }

    async fn set(&mut self, _k: Key, _v: Resource<OutgoingValue>, _ttl_ms: Option<u32>) -> anyhow::Result<Resource<FutureResult>> {
        unimplemented!("set")
    }

    async fn get_or_set(&mut self, _k: Key) -> anyhow::Result<Resource<FutureGetOrSetResult>> {
        unimplemented!("get_or_set")
    }

    async fn delete(&mut self, _k: Key) -> anyhow::Result<Resource<FutureResult>> {
        unimplemented!("delete")
    }
}
