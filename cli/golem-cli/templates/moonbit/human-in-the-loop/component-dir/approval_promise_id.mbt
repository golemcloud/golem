///|
/// A schema-supported representation of the full host promise identity
#derive.golem_schema
pub(all) struct ApprovalPromiseId {
  agent_id_high_bits : UInt64
  agent_id_low_bits : UInt64
  fingerprint_high_bits : UInt64
  fingerprint_low_bits : UInt64
  oplog_idx : UInt64
}

///|
fn ApprovalPromiseId::from_host(id : @api.PromiseId) -> ApprovalPromiseId {
  {
    agent_id_high_bits: id.agent_id.uuid.high_bits,
    agent_id_low_bits: id.agent_id.uuid.low_bits,
    fingerprint_high_bits: id.fingerprint.high_bits,
    fingerprint_low_bits: id.fingerprint.low_bits,
    oplog_idx: id.oplog_idx,
  }
}

///|
fn ApprovalPromiseId::to_host(self : Self) -> @api.PromiseId {
  {
    agent_id: {
      uuid: { high_bits: self.agent_id_high_bits, low_bits: self.agent_id_low_bits },
    },
    fingerprint: {
      high_bits: self.fingerprint_high_bits,
      low_bits: self.fingerprint_low_bits,
    },
    oplog_idx: self.oplog_idx,
  }
}
