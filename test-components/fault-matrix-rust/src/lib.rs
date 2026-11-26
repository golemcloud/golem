//! The Rust fixture of the streaming fault matrix.
//!
//! It implements the fixture contract documented in `integration-tests/src/fault_matrix.rs`.
//! Record `seq` of run `run` is the number `run * 100_000 + seq`, for `seq` in 1..=count.

use golem_rust::agentic::{AgentStream, OutputStream, StreamError, spawn_local};
use golem_rust::{
    PromiseId, agent_definition, agent_implementation, endpoint, tool_definition,
    tool_implementation,
};

fn record(run: u32, seq: u32) -> u32 {
    run * 100_000 + seq
}

#[tool_definition(version = "1.0.0")]
pub trait FaultMatrix {
    async fn produce(
        &self,
        run: u32,
        count: u32,
        pause_after: u32,
        gate: PromiseId,
        stdout: OutputStream,
    ) -> u32;
}

struct FaultMatrixImpl;

#[tool_implementation]
impl FaultMatrix for FaultMatrixImpl {
    async fn produce(
        &self,
        run: u32,
        count: u32,
        pause_after: u32,
        gate: PromiseId,
        mut stdout: OutputStream,
    ) -> u32 {
        let mut written = 0;
        for seq in 1..=count {
            if seq == pause_after + 1 {
                golem_rust::await_promise(&gate).await;
            }
            if stdout
                .write(record(run, seq).to_be_bytes().to_vec())
                .await
                .is_err()
            {
                break;
            }
            written = seq;
        }
        let _ = stdout.finish().await;
        written
    }
}

#[agent_definition(mount = "/fault-matrix/{name}")]
pub trait FaultMatrixFixture {
    fn new(name: String) -> Self;

    fn create_gate(&self) -> PromiseId;

    fn produce(&self, run: u32, count: u32, pause_after: u32, gate: PromiseId) -> AgentStream<u32>;

    #[endpoint(put = "/numbered/{run}/{count}?delay_ms={delay_ms}")]
    fn produce_paced(&self, run: u32, count: u32, delay_ms: u64) -> AgentStream<u32>;

    fn produce_then_fail(&self, run: u32, fail_after: u32) -> AgentStream<u32>;

    async fn consume(&self, input: AgentStream<u32>) -> Vec<u32>;

    async fn consume_then_drop(&self, input: AgentStream<u32>, keep: u32) -> Vec<u32>;

    async fn pipe(&self, target: String, run: u32, count: u32, keep: u32) -> Vec<u32>;

    async fn read_remote(
        &self,
        target: String,
        run: u32,
        count: u32,
        pause_after: u32,
        gate: PromiseId,
    ) -> Vec<u32>;
}

struct FaultMatrixFixtureImpl;

#[agent_implementation]
impl FaultMatrixFixture for FaultMatrixFixtureImpl {
    fn new(_name: String) -> Self {
        Self
    }

    fn create_gate(&self) -> PromiseId {
        golem_rust::create_promise()
    }

    fn produce(&self, run: u32, count: u32, pause_after: u32, gate: PromiseId) -> AgentStream<u32> {
        let (mut writer, output) = AgentStream::new();
        spawn_local(async move {
            for seq in 1..=count {
                if seq == pause_after + 1 {
                    golem_rust::await_promise(&gate).await;
                }
                if writer.write_one(record(run, seq)).await.is_err() {
                    return;
                }
            }
            let _ = writer.close().await;
        });
        output
    }

    fn produce_paced(&self, run: u32, count: u32, delay_ms: u64) -> AgentStream<u32> {
        let (mut writer, output) = AgentStream::new();
        spawn_local(async move {
            for seq in 1..=count {
                golem_rust::wasip3::clocks::monotonic_clock::wait_for(
                    delay_ms.saturating_mul(1_000_000),
                )
                .await;
                if writer.write_one(record(run, seq)).await.is_err() {
                    return;
                }
            }
            let _ = writer.close().await;
        });
        output
    }

    fn produce_then_fail(&self, run: u32, fail_after: u32) -> AgentStream<u32> {
        let (mut writer, output) = AgentStream::new();
        spawn_local(async move {
            for seq in 1..=fail_after {
                if writer.write_one(record(run, seq)).await.is_err() {
                    return;
                }
            }
            let error = golem_rust::schema::try_into_typed_schema_value(&"the producer failed".to_string())
                .expect("a string has a schema");
            let _ = writer.finish(Err(StreamError::Failed(Box::new(error)))).await;
        });
        output
    }

    async fn consume(&self, input: AgentStream<u32>) -> Vec<u32> {
        input.collect().await.expect("the input stream failed")
    }

    async fn consume_then_drop(&self, mut input: AgentStream<u32>, keep: u32) -> Vec<u32> {
        let mut records = Vec::new();
        while records.len() < keep as usize {
            match input.next().await {
                Some(record) => records.push(record.expect("the input stream failed")),
                None => break,
            }
        }
        drop(input);
        records
    }

    async fn pipe(&self, target: String, run: u32, count: u32, keep: u32) -> Vec<u32> {
        let (mut writer, input) = AgentStream::new();
        spawn_local(async move {
            for seq in 1..=count {
                if writer.write_one(record(run, seq)).await.is_err() {
                    return;
                }
            }
            let _ = writer.close().await;
        });
        let target = FaultMatrixFixtureClient::upsert(target);
        if keep >= count {
            target.consume(input).await
        } else {
            target.consume_then_drop(input, keep).await
        }
    }

    async fn read_remote(
        &self,
        target: String,
        run: u32,
        count: u32,
        pause_after: u32,
        gate: PromiseId,
    ) -> Vec<u32> {
        FaultMatrixFixtureClient::upsert(target)
            .produce(run, count, pause_after, gate)
            .await
            .collect()
            .await
            .expect("the remote output stream failed")
    }
}
