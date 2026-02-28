///|
fn init {
  let _ = lexmatch "" with longest {
    (("[0-9]{2}" as _hour_str) ":" ("[0-9]{2}" as _minute_str)) => 1
    (
      ("[0-9]{2}" as _hour_str)
      ":"
      ("[0-9]{2}" as _minute_str)
      ":"
      ("[0-9]{2}" as _second_str)
    ) => 2
    (
      ("[0-9]{2}" as _hour_str)
      ":"
      ("[0-9]{2}" as _minute_str)
      ":"
      ("[0-9]{2}" as _second_str)
      "."
      ("[0-9]{1,9}" as _nanosecond_str)
    ) => 3
    _ => 4
  }

}
