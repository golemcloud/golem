// The MoonBit fixture of the streaming fault matrix. It implements the fixture contract
// documented in integration-tests/src/fault_matrix.rs. Record `seq` of run `run` is the number
// `run * 100_000 + seq`, for `seq` in 1..=count.

///|
/// The schema shape of `golem:core/types@2.0.0` component-id.
#derive.golem_schema
pub(all) struct GateComponentId {
  uuid : @schema.Uuid
}

///|
/// The schema shape of `golem:core/types@2.0.0` agent-id.
#derive.golem_schema
pub(all) struct GateAgentId {
  component_id : GateComponentId
  agent_id : String
}

///|
/// The schema shape of `golem:core/types@2.0.0` promise-id.
#derive.golem_schema
pub(all) struct Gate {
  agent_id : GateAgentId
  fingerprint : @schema.Uuid
  oplog_idx : UInt64
}

///|
fn Gate::from_host(id : @types.PromiseId) -> Gate {
  {
    agent_id: {
      component_id: {
        uuid: @schema.Uuid::from_wit(id.agent_id.component_id.uuid),
      },
      agent_id: id.agent_id.agent_id,
    },
    fingerprint: @schema.Uuid::from_wit(id.fingerprint),
    oplog_idx: id.oplog_idx,
  }
}

///|
fn Gate::to_host(self : Gate) -> @types.PromiseId {
  {
    agent_id: {
      component_id: { uuid: self.agent_id.component_id.uuid.to_wit(), },
      agent_id: self.agent_id.agent_id,
    },
    fingerprint: self.fingerprint.to_wit(),
    oplog_idx: self.oplog_idx,
  }
}

///|
let unbounded : UInt32 = 0xFFFFFFFFU

///|
fn record(run : UInt32, seq : UInt32) -> UInt32 {
  run * 100000U + seq
}

///|
fn records(run : UInt32, count : UInt32) -> Array[UInt32] {
  let values : Array[UInt32] = []
  for seq = 1U; seq <= count; seq = seq + 1U {
    values.push(record(run, seq))
  }
  values
}

///|
fn records_stream(values : Array[UInt32]) -> @schema.AgentStream[UInt32] {
  @schema.AgentStream::produce(async fn(writer) {
    let _ = writer.write_all(values)
  })
}

///|
async fn collect(
  input : @schema.AgentStream[UInt32],
  keep : UInt32,
) -> Array[UInt32] {
  defer input.drop()
  let values : Array[UInt32] = []
  while values.length().reinterpret_as_uint() < keep {
    match input.read() {
      Some(value) => values.push(value)
      None => break
    }
  }
  values
}

///|
#derive.agent
struct FaultMatrixFixture {
  name : String
}

///|
fn FaultMatrixFixture::new(name : String) -> FaultMatrixFixture {
  { name, }
}

///|
pub fn FaultMatrixFixture::create_gate(self : Self) -> Gate {
  let _ = self
  Gate::from_host(@api.create_promise())
}

///|
pub fn FaultMatrixFixture::produce(
  self : Self,
  run : UInt32,
  count : UInt32,
  pause_after : UInt32,
  gate : Gate,
) -> @schema.AgentStream[UInt32] {
  let _ = self
  @schema.AgentStream::produce(async fn(writer) {
    for seq = 1U; seq <= count; seq = seq + 1U {
      if seq == pause_after + 1U {
        let _ = @api.await_promise(gate.to_host())
      }
      if writer.write_one(record(run, seq)) is @schema.PeerDropped {
        return
      }
    }
  })
}

///|
pub fn FaultMatrixFixture::produce_then_fail(
  self : Self,
  run : UInt32,
  fail_after : UInt32,
) -> @schema.AgentStream[UInt32] {
  let _ = self
  @schema.AgentStream::produce(async fn(writer) {
    if writer.write_all(records(run, fail_after)) is @schema.PeerDropped {
      return
    }
    fail("the fixture producer failed")
  })
}

///|
pub async fn FaultMatrixFixture::consume(
  self : Self,
  input : @schema.AgentStream[UInt32],
) -> Array[UInt32] {
  let _ = self
  collect(input, unbounded)
}

///|
pub async fn FaultMatrixFixture::consume_then_drop(
  self : Self,
  input : @schema.AgentStream[UInt32],
  keep : UInt32,
) -> Array[UInt32] {
  let _ = self
  collect(input, keep)
}

///|
pub async fn FaultMatrixFixture::pipe(
  self : Self,
  target : String,
  run : UInt32,
  count : UInt32,
  keep : UInt32,
) -> Array[UInt32] {
  let _ = self
  FaultMatrixFixtureClient::scoped(target, async fn(client) {
    let input = records_stream(records(run, count))
    if keep >= count {
      client.consume(input)
    } else {
      client.consume_then_drop(input, keep)
    }
  })
}

///|
pub async fn FaultMatrixFixture::read_remote(
  self : Self,
  target : String,
  run : UInt32,
  count : UInt32,
  pause_after : UInt32,
  gate : Gate,
) -> Array[UInt32] {
  let _ = self
  FaultMatrixFixtureClient::scoped(target, async fn(client) {
    collect(client.produce(run, count, pause_after, gate), unbounded)
  })
}
