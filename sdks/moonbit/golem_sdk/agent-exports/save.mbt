///|
priv suberror SnapshotSaveError {
  SnapshotSaveError(String)
} derive(Debug)

///|
/// Saves the component's state into a snapshot with a versioned envelope.
/// JSON path: {"version":1, "principal":<json>, "state":<json>}
/// Binary path: [0x02][u32 BE principal_len][principal_json_bytes][state_bytes]
#warnings("-unused_async")
pub async fn save(
  _background_group : @async-core.TaskGroup[Unit],
) -> @host.Snapshot {
  match @agents.get_agent_instance() {
    Some(instance) => {
      if instance.multipart_snapshottable is Some(saver) {
        let parts = match saver.save_snapshot_parts() {
          Ok(parts) => parts
          Err(message) => raise SnapshotSaveError(message)
        }
        return match
          @agents.encode_multipart_snapshot(parts, instance.principal) {
          Ok((bytes, mime_type)) =>
            { payload: bytes.to_fixedarray(), mime_type, }
          Err(message) => raise SnapshotSaveError(message)
        }
      }
      match (instance.snapshottable, instance.snapshot_format) {
        (Some(s), Some(@agents.SnapshotFormat::Json)) => {
          let state_bytes = s.save_snapshot()
          let state_str = @utf8.decode(state_bytes)
          let state_json = @json.parse(state_str) catch {
            _ => Json::string(state_str)
          }
          let principal_json = @agents.principal_to_json(instance.principal)
          let envelope : Map[String, Json] = Map([])
          envelope["version"] = Json::number(1.0)
          envelope["principal"] = principal_json
          envelope["state"] = state_json
          let envelope_str = Json::object(envelope).stringify()
          let envelope_bytes = @utf8.encode(envelope_str)
          {
            payload: envelope_bytes.to_fixedarray(),
            mime_type: "application/json",
          }
        }
        (Some(s), Some(@agents.SnapshotFormat::Binary)) => {
          let state_bytes = s.save_snapshot()
          let principal_json = @agents.principal_to_json(instance.principal)
          let principal_str = principal_json.stringify()
          let principal_bytes = @utf8.encode(principal_str)
          let principal_len = principal_bytes.length()
          let total_len = 1 + 4 + principal_len + state_bytes.length()
          let result = FixedArray::make(total_len, b'\x00')
          result[0] = b'\x02'
          result[1] = ((principal_len >> 24) & 0xFF).to_byte()
          result[2] = ((principal_len >> 16) & 0xFF).to_byte()
          result[3] = ((principal_len >> 8) & 0xFF).to_byte()
          result[4] = (principal_len & 0xFF).to_byte()
          for i in 0..<principal_len {
            result[5 + i] = principal_bytes[i]
          }
          for i in 0..<state_bytes.length() {
            result[5 + principal_len + i] = state_bytes[i]
          }
          { payload: result, mime_type: "application/octet-stream", }
        }
        (Some(s), None) => {
          let data = s.save_snapshot()
          {
            payload: data.to_fixedarray(),
            mime_type: "application/octet-stream",
          }
        }
        _ => { payload: [], mime_type: "application/octet-stream", }
      }
    }
    None => { payload: [], mime_type: "application/octet-stream", }
  }
}
