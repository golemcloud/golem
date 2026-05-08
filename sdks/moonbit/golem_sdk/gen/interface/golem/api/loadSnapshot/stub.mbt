///|
priv suberror SnapshotLoadError {
  SnapshotLoadError(String)
} derive(Show)

///|
/// Tries to load a snapshot, parsing the versioned envelope and restoring agent state.
/// Supports both JSON and binary envelopes, as well as snapshot-based updates
/// on uninitialized agents (reads GOLEM_AGENT_ID from environment).
pub fn load(snapshot : @host.Snapshot) -> Result[Unit, String] {
  if snapshot.payload.length() == 0 {
    return Ok(())
  }
  let result = parse_snapshot_envelope(snapshot) catch {
    SnapshotLoadError(msg) => return Err(msg)
  }
  let principal = result.0
  let state_bytes = result.1
  match @agents.get_agent_instance() {
    Some(instance) =>
      match instance.snapshottable {
        Some(s) => s.load_snapshot(state_bytes)
        None => Err("Agent does not support snapshots")
      }
    None => {
      // Snapshot-based update on a fresh component instance.
      // Read GOLEM_AGENT_ID from environment and reconstruct the agent.
      let agent_id_str = get_golem_agent_id() catch {
        SnapshotLoadError(msg) => return Err(msg)
      }
      match @agentHost.parse_agent_id(agent_id_str) {
        Ok((type_name, ctor_params, _phantom_id)) =>
          match
            @agents.construct_and_set_instance(
              type_name, ctor_params, principal,
            ) {
            Ok(_) =>
              match @agents.get_agent_instance() {
                Some(instance) =>
                  match instance.snapshottable {
                    Some(s) => s.load_snapshot(state_bytes)
                    None => Err("Agent does not support snapshots")
                  }
                None => Err("Failed to construct agent instance")
              }
            Err(e) => Err("Failed to construct agent: \{e}")
          }
        Err(e) => Err("Failed to parse agent ID: \{e}")
      }
    }
  }
}

///|
fn get_golem_agent_id() -> String raise SnapshotLoadError {
  let env = @environment.get_environment()
  for pair in env {
    if pair.0 == "GOLEM_AGENT_ID" {
      return pair.1
    }
  }
  raise SnapshotLoadError("GOLEM_AGENT_ID environment variable is not set")
}

///|
fn slice_fixedarray_to_bytes(arr : FixedArray[Byte], start : Int) -> Bytes {
  let slice : Array[Byte] = []
  for i in start..<arr.length() {
    slice.push(arr[i])
  }
  Bytes::from_array(slice)
}

///|
fn parse_snapshot_envelope(
  snapshot : @host.Snapshot,
) -> (@common.Principal, Bytes) raise SnapshotLoadError {
  if snapshot.mime_type == "application/json" {
    let chars : Array[Char] = []
    for i in 0..<snapshot.payload.length() {
      chars.push(Char::from_int(snapshot.payload[i].to_int()))
    }
    let json_str = String::from_array(chars)
    let json = @json.parse(json_str) catch {
      e => raise SnapshotLoadError("Failed to parse snapshot JSON: \{e}")
    }
    let obj = match json {
      Object(m) => m
      _ => raise SnapshotLoadError("Expected JSON object in snapshot envelope")
    }
    let principal = match obj.get("principal") {
      Some(p) =>
        @agents.principal_from_json(p) catch {
          e => raise SnapshotLoadError("Failed to parse principal: \{e}")
        }
      None => @common.Principal::Anonymous
    }
    let state_json = match obj.get("state") {
      Some(s) => s
      None => raise SnapshotLoadError("Missing 'state' in snapshot envelope")
    }
    let state_str = state_json.stringify()
    let state_bytes = Bytes::from_array(
      state_str.to_array().map(fn(c) { c.to_int().to_byte() }),
    )
    (principal, state_bytes)
  } else {
    let data = snapshot.payload
    if data.length() == 0 {
      raise SnapshotLoadError("Empty binary snapshot")
    }
    let version = data[0].to_int()
    match version {
      1 => {
        let state_bytes = slice_fixedarray_to_bytes(data, 1)
        (@common.Principal::Anonymous, state_bytes)
      }
      2 => {
        if data.length() < 5 {
          raise SnapshotLoadError(
            "Binary snapshot too short for version 2 header",
          )
        }
        let principal_len = (data[1].to_int() << 24) |
          (data[2].to_int() << 16) |
          (data[3].to_int() << 8) |
          data[4].to_int()
        if data.length() < 5 + principal_len {
          raise SnapshotLoadError(
            "Binary snapshot too short for principal data",
          )
        }
        let principal_chars : Array[Char] = []
        for i in 0..<principal_len {
          principal_chars.push(Char::from_int(data[5 + i].to_int()))
        }
        let principal_str = String::from_array(principal_chars)
        let principal_json = @json.parse(principal_str) catch {
          e =>
            raise SnapshotLoadError(
              "Failed to parse principal JSON from binary envelope: \{e}",
            )
        }
        let principal = @agents.principal_from_json(principal_json) catch {
          e => raise SnapshotLoadError("Failed to deserialize principal: \{e}")
        }
        let state_bytes = slice_fixedarray_to_bytes(data, 5 + principal_len)
        (principal, state_bytes)
      }
      other =>
        raise SnapshotLoadError("Unsupported binary snapshot version: \{other}")
    }
  }
}
