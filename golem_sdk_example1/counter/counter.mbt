///|
struct Counter {
  name : String
  mut value : UInt64
}

///|
fn Counter::new_raw(
  input : @common.DataValue,
) -> Result[&@agents.RawAgent, @common.AgentError] {
  Ok({ name: "TODO", value: 0 })
}

///|
pub fn Counter::increment(self : Self) -> Unit {
  self.value += 1
}

///|
impl @agents.RawAgent for Counter with invoke(self, method_name, input) {
  match method_name {
    "increment" => {
      let result = self.increment()
      Ok(@common.DataValue::Tuple([]))
    }
    _ =>
      @agents.panic_with_message(
        "Counter::invoke",
        "Invalid method name: \{method_name}",
      )
  }
}

///|
fn init {
  @agents.register_agent({
    name: "Counter",
    agent_type: {
      type_name: "Counter",
      description: "Counter agent in MoonBit",
      constructor_: {
        name: None,
        description: "Constructs the counter",
        prompt_hint: None,
        input_schema: @common.DataSchema::Tuple([
          (
            "name",
            @common.ElementSchema::ComponentModel({
              nodes: [
                {
                  name: None,
                  owner: None,
                  type_: @types.WitTypeNode::PrimStringType,
                },
              ],
            }),
          ),
        ]),
      },
      methods: [],
      dependencies: [],
    },
    construct: Counter::new_raw,
  })
}

///|
fn main {

}

// reexports
// pub using @gen { cabi_realloc, wasmExportDiscoverAgentTypes, wasmExportDiscoverAgentTypesPostReturn, wasmExportGetDefinition, wasmExportGetDefinitionPostReturn, wasmExportInitialize, wasmExportInitializePostReturn, wasmExportInvoke, wasmExportInvokePostReturn, wasmExportLoad, wasmExportLoadPostReturn, wasmExportSave, wasmExportSavePostReturn }

///|
pub fn cabi_realloc(
  src_offset : Int,
  src_size : Int,
  dst_alignment : Int,
  dst_size : Int,
) -> Int {
  @gen.mbt_ffi_cabi_realloc(src_offset, src_size, dst_alignment, dst_size)
}

///|
pub fn wasmExportInitialize(
  p0 : Int,
  p1 : Int,
  p2 : Int,
  p3 : Int,
  p4 : Int,
) -> Int {
  @gen.wasmExportInitialize(p0, p1, p2, p3, p4)
}

///|
pub fn wasmExportInitializePostReturn(p0 : Int) -> Unit {
  @gen.wasmExportInitializePostReturn(p0)
}

///|
pub fn wasmExportInvoke(
  p0 : Int,
  p1 : Int,
  p2 : Int,
  p3 : Int,
  p4 : Int,
) -> Int {
  @gen.wasmExportInvoke(p0, p1, p2, p3, p4)
}

///|
pub fn wasmExportInvokePostReturn(p0 : Int) -> Unit {
  @gen.wasmExportInvokePostReturn(p0)
}

///|
pub fn wasmExportGetDefinition() -> Int {
  @gen.wasmExportGetDefinition()
}

///|
pub fn wasmExportGetDefinitionPostReturn(p0 : Int) -> Unit {
  @gen.wasmExportGetDefinitionPostReturn(p0)
}

///|
pub fn wasmExportDiscoverAgentTypes() -> Int {
  @gen.wasmExportDiscoverAgentTypes()
}

///|
pub fn wasmExportDiscoverAgentTypesPostReturn(p0 : Int) -> Unit {
  @gen.wasmExportDiscoverAgentTypesPostReturn(p0)
}

///|
pub fn wasmExportLoad(p0 : Int, p1 : Int) -> Int {
  @gen.wasmExportLoad(p0, p1)
}

///|
pub fn wasmExportLoadPostReturn(p0 : Int) -> Unit {
  @gen.wasmExportLoadPostReturn(p0)
}

///|
pub fn wasmExportSave() -> Int {
  @gen.wasmExportSave()
}

///|
pub fn wasmExportSavePostReturn(p0 : Int) -> Unit {
  @gen.wasmExportSavePostReturn(p0)
}
