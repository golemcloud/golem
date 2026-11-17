///|
priv suberror SnapshotSaveError {
  SnapshotSaveError(String)
} derive(Debug)

///|
/// The version 2 JSON envelope: {"version":2,"state":<state>}.
fn json_snapshot_envelope(state_bytes : Bytes) -> FixedArray[Byte] {
  let state_str = @utf8.decode(state_bytes)
  let state_json = @json.parse(state_str) catch { _ => Json::string(state_str) }
  let envelope : Map[String, Json] = Map([])
  envelope["version"] = Json::number(2.0)
  envelope["state"] = state_json
  let envelope_str = Json::object(envelope).stringify()
  @utf8.encode(envelope_str).to_fixedarray()
}

///|
/// The version 3 binary envelope: the byte 0x03, then the state.
fn binary_snapshot_envelope(state_bytes : Bytes) -> FixedArray[Byte] {
  let result = FixedArray::make(1 + state_bytes.length(), b'\x03')
  for i in 0..<state_bytes.length() {
    result[1 + i] = state_bytes[i]
  }
  result
}

///|
/// Saves the component's state into a snapshot with a versioned envelope.
/// JSON path: {"version":2, "state":<json>}
/// Binary path: [0x03][state_bytes]
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
        return match @agents.encode_multipart_snapshot(parts) {
          Ok((bytes, mime_type)) =>
            { payload: bytes.to_fixedarray(), mime_type, }
          Err(message) => raise SnapshotSaveError(message)
        }
      }
      match (instance.snapshottable, instance.snapshot_format) {
        (Some(s), Some(@agents.SnapshotFormat::Json)) => {
          let state_bytes = s.save_snapshot()
          {
            payload: json_snapshot_envelope(state_bytes),
            mime_type: "application/json",
          }
        }
        (Some(s), Some(@agents.SnapshotFormat::Binary)) => {
          let state_bytes = s.save_snapshot()
          {
            payload: binary_snapshot_envelope(state_bytes),
            mime_type: "application/octet-stream",
          }
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
