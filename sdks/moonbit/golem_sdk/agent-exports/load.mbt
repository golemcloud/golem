///|
priv suberror SnapshotLoadError {
  SnapshotLoadError(String)
} derive(Debug)

///|
/// Tries to load a snapshot, parsing the versioned envelope and restoring agent state.
/// Supports both JSON and binary envelopes on an uninitialized component.
/// `constructor_` holds the agent's constructor parameters, which the restore
/// code receives in its `SnapshotRestoreContext`.
pub async fn load(
  snapshot : @host.Snapshot,
  constructor_ : @types.SchemaValueTree,
  _background_group : @async-core.TaskGroup[Unit],
) -> Result[Unit, String] {
  load_with_environment(snapshot, constructor_, @environment.get_environment())
}

///|
async fn load_with_environment(
  snapshot : @host.Snapshot,
  constructor_ : @types.SchemaValueTree,
  env : Array[(String, String)],
) -> Result[Unit, String] {
  let (state_bytes, agent_id, agent_type) = read_restore_target(snapshot, env) catch {
    SnapshotLoadError(msg) => {
      @model.drop_wit_capabilities(constructor_)
      return Err(msg)
    }
  }
  let identity_fields = @agents.decode_snapshot_constructor(
    agent_type, constructor_,
  ) catch {
    e => return Err("Failed to decode constructor parameters: \{Repr(e)}")
  }
  @agents.restore_and_set_instance(agent_type, state_bytes, {
    agent_id,
    agent_type,
    identity_fields,
  })
}

///|
fn read_restore_target(
  snapshot : @host.Snapshot,
  env : Array[(String, String)],
) -> (@agents.SnapshotData, @schema.AgentId, String) raise SnapshotLoadError {
  if @agents.get_agent_instance() is Some(_) {
    raise SnapshotLoadError("Agent instance is already initialized")
  }
  if snapshot.payload.length() == 0 {
    raise SnapshotLoadError("Snapshot is empty")
  }
  let state_bytes = parse_snapshot_envelope(snapshot)
  let agent_id_text = get_golem_env_var(env, "GOLEM_AGENT_ID")
  let agent_id = @schema.AgentId::parse(agent_id_text) catch {
    e => raise SnapshotLoadError("Failed to parse agent ID: \{Repr(e)}")
  }
  let agent_type = get_golem_env_var(env, "GOLEM_AGENT_TYPE")
  (state_bytes, agent_id, agent_type)
}

///|
/// Finds a variable that Golem sets in the environment that the agent can read.
/// The host shows such a variable only when the agent's permission card allows
/// the agent to read it.
fn get_golem_env_var(
  env : Array[(String, String)],
  name : String,
) -> String raise SnapshotLoadError {
  for pair in env {
    if pair.0 == name {
      return pair.1
    }
  }
  raise SnapshotLoadError(
    "\{name} is not readable: the agent's permission card must allow env(?agent) @ ?agent : read : \{name}",
  )
}

///|
fn slice_fixedarray_to_bytes(arr : FixedArray[Byte], start : Int32) -> Bytes {
  let slice : Array[Byte] = []
  for i in start..<arr.length() {
    slice.push(arr[i])
  }
  Bytes::from_array(slice)
}

///|
fn parse_snapshot_envelope(
  snapshot : @host.Snapshot,
) -> @agents.SnapshotData raise SnapshotLoadError {
  if snapshot.mime_type.split(";").next() is Some(kind) &&
    kind.trim().to_string().to_lower() == "multipart/mixed" {
    return match
      @agents.decode_multipart_snapshot(
        Bytes::from_fixedarray(snapshot.payload),
        snapshot.mime_type,
      ) {
      Ok(parts) => @agents.SnapshotData::Multipart(parts)
      Err(message) => raise SnapshotLoadError(message)
    }
  }
  if snapshot.mime_type == "application/json" {
    let json_str = @utf8.decode(Bytes::from_fixedarray(snapshot.payload)) catch {
      _ => raise SnapshotLoadError("Invalid snapshot JSON UTF-8")
    }
    let json = @json.parse(json_str) catch {
      e => raise SnapshotLoadError("Failed to parse snapshot JSON: \{e}")
    }
    let obj = match json {
      Object(m) => m
      _ => raise SnapshotLoadError("Expected JSON object in snapshot envelope")
    }
    let version = match obj.get("version") {
      Some(version) =>
        match version {
          Number(version, ..) => version
          _ => raise SnapshotLoadError("Invalid JSON snapshot version")
        }
      None => raise SnapshotLoadError("Missing 'version' in snapshot envelope")
    }
    if version != 1.0 && version != 2.0 {
      raise SnapshotLoadError("Unsupported JSON snapshot version: \{version}")
    }
    // A 1.6 envelope (version 1) carries a principal: it is checked as before and dropped.
    if version == 1.0 {
      let principal_json = match obj.get("principal") {
        Some(principal) => principal
        None =>
          raise SnapshotLoadError("Missing 'principal' in snapshot envelope")
      }
      @agents.validate_legacy_principal_json(principal_json) catch {
        e =>
          raise SnapshotLoadError(
            "Failed to parse principal: \{@debug.to_string(e)}",
          )
      }
    }
    let state_json = match obj.get("state") {
      Some(s) => s
      None => raise SnapshotLoadError("Missing 'state' in snapshot envelope")
    }
    let state_str = state_json.stringify()
    @agents.SnapshotData::Json(@utf8.encode(state_str))
  } else {
    let data = snapshot.payload
    if data.length() == 0 {
      raise SnapshotLoadError("Empty binary snapshot")
    }
    let version = data[0].to_int()
    match version {
      // Version 1 is the 1.6 form without a principal; version 3 is the 2.0 form.
      1 | 3 => {
        let state_bytes = slice_fixedarray_to_bytes(data, 1)
        @agents.SnapshotData::Binary(state_bytes)
      }
      2 => {
        if data.length() < 5 {
          raise SnapshotLoadError(
            "Binary snapshot too short for version 2 header",
          )
        }
        if data[1].to_int() > 0x7F {
          raise SnapshotLoadError(
            "Binary snapshot principal data exceeds supported size",
          )
        }
        let principal_len = (data[1].to_int() << 24) |
          (data[2].to_int() << 16) |
          (data[3].to_int() << 8) |
          data[4].to_int()
        if principal_len > data.length() - 5 {
          raise SnapshotLoadError(
            "Binary snapshot too short for principal data",
          )
        }
        let principal_bytes = Bytes::from_fixedarray(data)[5:5 + principal_len]
        let principal_str = @utf8.decode(principal_bytes) catch {
          _ => raise SnapshotLoadError("Invalid principal JSON UTF-8")
        }
        let principal_json = @json.parse(principal_str) catch {
          e =>
            raise SnapshotLoadError(
              "Failed to parse principal JSON from binary envelope: \{e}",
            )
        }
        // A 1.6 envelope: its principal is checked as before and dropped.
        @agents.validate_legacy_principal_json(principal_json) catch {
          e =>
            raise SnapshotLoadError(
              "Failed to deserialize principal: \{@debug.to_string(e)}",
            )
        }
        let state_bytes = slice_fixedarray_to_bytes(data, 5 + principal_len)
        @agents.SnapshotData::Binary(state_bytes)
      }
      other =>
        raise SnapshotLoadError("Unsupported binary snapshot version: \{other}")
    }
  }
}
