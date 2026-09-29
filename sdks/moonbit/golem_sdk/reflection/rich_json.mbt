///|
fn days_from_civil(year : Int, month : Int, day : Int) -> Int {
  let y = if month <= 2 { year - 1 } else { year }
  let era = if y >= 0 { y / 400 } else { (y - 399) / 400 }
  let yoe = y - era * 400
  let m = if month > 2 { month - 3 } else { month + 9 }
  let doy = (153 * m + 2) / 5 + day - 1
  let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy
  era * 146097 + doe - 719468
}

///|
fn civil_from_days(days : Int) -> (Int, Int, Int) {
  let z = days + 719468
  let era = if z >= 0 { z / 146097 } else { (z - 146096) / 146097 }
  let doe = z - era * 146097
  let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365
  let y = yoe + era * 400
  let doy = doe - (365 * yoe + yoe / 4 - yoe / 100)
  let mp = (5 * doy + 2) / 153
  let day = doy - (153 * mp + 2) / 5 + 1
  let month = if mp < 10 { mp + 3 } else { mp - 9 }
  (if month <= 2 { y + 1 } else { y }, month, day)
}

///|
fn datetime_to_json(value : @types.Datetime) -> String raise ReflectError {
  guard value.nanoseconds < 1000000000U &&
    value.seconds >= -62167219200L &&
    value.seconds <= 253402300799L else {
    raise Json("datetime is outside canonical year range 0000..9999")
  }
  let days = if value.seconds < 0L {
    (value.seconds - 86399L) / 86400L
  } else {
    value.seconds / 86400L
  }
  let second_of_day = (value.seconds - days * 86400L).to_int()
  let (year, month, day) = civil_from_days(days.to_int())
  let hour = second_of_day / 3600
  let minute = second_of_day % 3600 / 60
  let second = second_of_day % 60
  let mut result = "\{year}".pad_start(4, '0') +
    "-" +
    "\{month}".pad_start(2, '0') +
    "-" +
    "\{day}".pad_start(2, '0') +
    "T" +
    "\{hour}".pad_start(2, '0') +
    ":" +
    "\{minute}".pad_start(2, '0') +
    ":" +
    "\{second}".pad_start(2, '0')
  if value.nanoseconds != 0U {
    let mut fraction = "\{value.nanoseconds}".pad_start(9, '0')
    while fraction.has_suffix("0") {
      fraction = fraction.view()[0:fraction.length() - 1].to_owned()
    }
    result += "." + fraction
  }
  result + "Z"
}

///|
fn datetime_from_json(text : String) -> @types.Datetime raise ReflectError {
  let matched = re"^([0-9]{4})-([0-9]{2})-([0-9]{2})T([0-9]{2}):([0-9]{2}):([0-9]{2})(?:\.([0-9]{1,9}))?Z$".execute(
    text,
  )
  guard matched is Some(m) else { raise Json("invalid UTC datetime") }
  let year = @string.parse_int(m.group(1).unwrap(), base=10) catch {
    _ => raise Json("invalid UTC datetime")
  }
  let month = @string.parse_int(m.group(2).unwrap(), base=10) catch {
    _ => raise Json("invalid UTC datetime")
  }
  let day = @string.parse_int(m.group(3).unwrap(), base=10) catch {
    _ => raise Json("invalid UTC datetime")
  }
  let hour = @string.parse_int(m.group(4).unwrap(), base=10) catch {
    _ => raise Json("invalid UTC datetime")
  }
  let minute = @string.parse_int(m.group(5).unwrap(), base=10) catch {
    _ => raise Json("invalid UTC datetime")
  }
  let second = @string.parse_int(m.group(6).unwrap(), base=10) catch {
    _ => raise Json("invalid UTC datetime")
  }
  guard month >= 1 &&
    month <= 12 &&
    day >= 1 &&
    day <= 31 &&
    hour <= 23 &&
    minute <= 59 &&
    second <= 59 else {
    raise Json("invalid UTC datetime")
  }
  let days = days_from_civil(year, month, day)
  guard civil_from_days(days) == (year, month, day) else {
    raise Json("invalid UTC datetime")
  }
  let nanos = match m.group(7) {
    Some(fraction) =>
      @string.parse_uint(fraction.to_owned().pad_end(9, '0').view(), base=10) catch {
        _ => raise Json("invalid UTC datetime")
      }
    None => 0U
  }
  {
    seconds: days.to_int64() * 86400L +
    hour.to_int64() * 3600L +
    minute.to_int64() * 60L +
    second.to_int64(),
    nanoseconds: nanos,
  }
}
