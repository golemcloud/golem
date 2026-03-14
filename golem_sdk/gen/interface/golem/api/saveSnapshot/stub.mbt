///|
/// Saves the component's state into a snapshot with a versioned envelope.
/// JSON path: {"version":1, "principal":<json>, "state":<json>}
/// Binary path: [0x02][u32 BE principal_len][principal_json_bytes][state_bytes]
pub fn save() -> @host.Snapshot {
  match @agents.get_agent_instance() {
    Some(instance) =>
      match (instance.snapshottable, instance.snapshot_format) {
        (Some(s), Some(@agents.SnapshotFormat::Json)) => {
          let state_bytes = s.save_snapshot()
          let state_chars : Array[Char] = []
          for i in 0..<state_bytes.length() {
            state_chars.push(Char::from_int(state_bytes[i].to_int()))
          }
          let state_str = String::from_array(state_chars)
          let state_json = @json.parse(state_str) catch {
            _ => Json::string(state_str)
          }
          let principal_json = @agents.principal_to_json(instance.principal)
          let envelope : Map[String, Json] = {}
          envelope["version"] = Json::number(1.0)
          envelope["principal"] = principal_json
          envelope["state"] = state_json
          let envelope_str = Json::object(envelope).stringify()
          let envelope_bytes = Bytes::from_array(
            envelope_str.to_array().map(fn(c) { c.to_int().to_byte() }),
          )
          {
            data: envelope_bytes.to_fixedarray(),
            mime_type: "application/json",
          }
        }
        (Some(s), Some(@agents.SnapshotFormat::Binary)) => {
          let state_bytes = s.save_snapshot()
          let principal_json = @agents.principal_to_json(instance.principal)
          let principal_str = principal_json.stringify()
          let principal_bytes = Bytes::from_array(
            principal_str.to_array().map(fn(c) { c.to_int().to_byte() }),
          )
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
          { data: result, mime_type: "application/octet-stream" }
        }
        (Some(s), None) => {
          let data = s.save_snapshot()
          { data: data.to_fixedarray(), mime_type: "application/octet-stream" }
        }
        _ => { data: [], mime_type: "application/octet-stream" }
      }
    None => { data: [], mime_type: "application/octet-stream" }
  }
}
