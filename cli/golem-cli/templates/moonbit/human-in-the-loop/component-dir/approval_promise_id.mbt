///|
/// A schema-supported representation of the full host promise identity
#derive.golem_schema
pub(all) struct ApprovalPromiseId {
  component_id_high_bits : UInt64
  component_id_low_bits : UInt64
  agent_id : String
  oplog_idx : UInt64
}

///|
fn ApprovalPromiseId::from_host(id : @api.PromiseId) -> ApprovalPromiseId {
  {
    component_id_high_bits: id.agent_id.component_id.uuid.high_bits,
    component_id_low_bits: id.agent_id.component_id.uuid.low_bits,
    agent_id: id.agent_id.agent_id,
    oplog_idx: id.oplog_idx,
  }
}

///|
fn ApprovalPromiseId::to_host(self : Self) -> @api.PromiseId {
  {
    agent_id: {
      component_id: {
        uuid: {
          high_bits: self.component_id_high_bits,
          low_bits: self.component_id_low_bits,
        },
      },
      agent_id: self.agent_id,
    },
    oplog_idx: self.oplog_idx,
  }
}
