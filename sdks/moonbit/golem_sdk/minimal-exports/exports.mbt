///|
/// Canonical result<list<_>, _>: success tag, null pointer, zero length.
pub fn discover() -> Int {
  let result = @gen.mbt_ffi_cabi_realloc(0, 0, 8, 16)
  store32(result, 0)
  store32(result + 4, 0)
  store32(result + 8, 0)
  result
}

///|
pub fn post_discover(result : Int) -> Unit {
  free(result)
}

///|
/// Transfer the UTF-16 input allocation into InvalidToolName. Post-return owns
/// both allocations; the string length remains in UTF-16 code units.
pub fn get(name : Int, length : Int) -> Int {
  let result = @gen.mbt_ffi_cabi_realloc(0, 0, 8, 16)
  store32(result, 1)
  store32(result + 4, 0)
  store32(result + 8, name)
  store32(result + 12, length)
  result
}

///|
pub fn post_get(result : Int) -> Unit {
  free(load32(result + 8))
  free(result)
}

///|
/// Source discovery prevents the host from selecting absent implementations.
pub extern "wasm" fn invoke(_args : Int) -> Int =
  #|(func (param i32) (result i32) unreachable)

///|
pub extern "wasm" fn definition() -> Int =
  #|(func (result i32) unreachable)

///|
pub fn post_definition(_result : Int) -> Unit {
  ()
}

///|
/// Immediate task-return leaves no suspended task and therefore no callback.
pub extern "wasm" fn callback(
  _event : Int,
  _waitable : Int,
  _code : Int,
) -> Int =
  #|(func (param i32 i32 i32) (result i32) unreachable)

///|
pub fn save() -> Int {
  let mime = "application/octet-stream"
  let ptr = string_ptr(mime)
  return_save(0, 0, ptr, mime.length())
  free(ptr)
  0
}

///|
pub fn load(
  payload : Int,
  _payload_length : Int,
  mime : Int,
  _mime_length : Int,
) -> Int {
  free(payload)
  free(mime)
  let error = "Component defines no agents"
  let ptr = string_ptr(error)
  return_load(1, ptr, error.length())
  free(ptr)
  0
}

///|
fn return_save(payload : Int, length : Int, mime : Int, mime_length : Int) = "[export]golem:api/save-snapshot@1.5.0" "[task-return]save"

///|
fn return_load(tag : Int, error : Int, length : Int) = "[export]golem:api/load-snapshot@1.5.0" "[task-return]load"

///|
extern "wasm" fn store32(ptr : Int, value : Int) =
  #|(func (param i32 i32) local.get 0 local.get 1 i32.store)

///|
extern "wasm" fn load32(ptr : Int) -> Int =
  #|(func (param i32) (result i32) local.get 0 i32.load)

///|
#owned(value)
extern "wasm" fn string_ptr(value : String) -> Int =
  #|(func (param i32) (result i32) local.get 0)

///|
fn free(ptr : Int) -> Unit {
  if ptr != 0 {
    ignore(@gen.mbt_ffi_cabi_realloc(ptr, 0, 1, 0))
  }
}
