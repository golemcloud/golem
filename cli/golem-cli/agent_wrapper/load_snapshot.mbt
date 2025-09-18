/// Tries to load a user-defined snapshot, setting up the worker's state based on it.
/// The function can return with a failure to indicate that the update is not possible.
pub fn load(bytes : FixedArray[Byte]) -> Result[Unit, String] {
      @loadSnapshot.load(bytes)
}

