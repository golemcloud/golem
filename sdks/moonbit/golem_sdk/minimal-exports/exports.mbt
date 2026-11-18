///|
/// Canonical result<list<_>, _>: success tag, null pointer, zero length.
pub fn discover() -> Int32 {
  let result = @gen.mbt_ffi_cabi_realloc(0, 0, 8, 16)
  store32(result, 0)
  store32(result + 4, 0)
  store32(result + 8, 0)
  result
}

///|
pub fn post_discover(result : Int32) -> Unit {
  free(result)
}

///|
/// Transfer the UTF-16 input allocation into InvalidToolName. Post-return owns
/// both allocations; the string length remains in UTF-16 code units.
pub fn get(name : Int32, length : Int32) -> Int32 {
  let result = @gen.mbt_ffi_cabi_realloc(0, 0, 8, 16)
  store32(result, 1)
  store32(result + 4, 0)
  store32(result + 8, name)
  store32(result + 12, length)
  result
}

///|
pub fn post_get(result : Int32) -> Unit {
  free(load32(result + 8))
  free(result)
}

///|
/// Source discovery prevents the host from selecting absent implementations.
pub extern "wasm" fn invoke(_args : Int32) -> Int32 =
  #|(func (param i32) (result i32) unreachable)

///|
/// Agent `initialize` and `invoke` take a name string and a schema value tree,
/// which flatten to five core parameters.
pub extern "wasm" fn invoke_agent(
  _name : Int32,
  _name_length : Int32,
  _value_nodes : Int32,
  _value_nodes_length : Int32,
  _root : Int32,
) -> Int32 =
  #|(func (param i32 i32 i32 i32 i32) (result i32) unreachable)

///|
pub extern "wasm" fn definition() -> Int32 =
  #|(func (result i32) unreachable)

///|
pub fn post_definition(_result : Int32) -> Unit {
  ()
}

///|
/// Immediate task-return leaves no suspended task and therefore no callback.
pub extern "wasm" fn callback(
  _event : Int32,
  _waitable : Int32,
  _code : Int32,
) -> Int32 =
  #|(func (param i32 i32 i32) (result i32) unreachable)

///|
pub fn save() -> Int32 {
  let mime = "application/octet-stream"
  let ptr = string_ptr(mime)
  return_save(0, 0, ptr, mime.length())
  free(ptr)
  0
}

///|
pub fn load(
  payload : Int32,
  _payload_length : Int32,
  mime : Int32,
  _mime_length : Int32,
) -> Int32 {
  free(payload)
  free(mime)
  let error = "Component defines no agents"
  let ptr = string_ptr(error)
  return_load(1, ptr, error.length())
  free(ptr)
  0
}

///|
fn return_save(
  payload : Int32,
  length : Int32,
  mime : Int32,
  mime_length : Int32,
) = "[export]golem:api/save-snapshot@1.5.0" "[task-return]save"

///|
fn return_load(tag : Int32, error : Int32, length : Int32) = "[export]golem:api/load-snapshot@1.5.0" "[task-return]load"

///|
extern "wasm" fn store32(ptr : Int32, value : Int32) =
  #|(func (param i32 i32) local.get 0 local.get 1 i32.store)

///|
extern "wasm" fn load32(ptr : Int32) -> Int32 =
  #|(func (param i32) (result i32) local.get 0 i32.load)

///|
#owned(value)
extern "wasm" fn string_ptr(value : String) -> Int32 =
  #|(func (param i32) (result i32) local.get 0)

///|
fn free(ptr : Int32) -> Unit {
  if ptr != 0 {
    ignore(@gen.mbt_ffi_cabi_realloc(ptr, 0, 1, 0))
  }
}
