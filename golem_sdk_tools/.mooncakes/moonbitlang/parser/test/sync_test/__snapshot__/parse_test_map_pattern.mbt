fn init {
  match x {
    {} => "record pattern"
    { "x": 1, 2: x, true: 42 as y } => "map pattern"
    { x: _, "x": _ } => "cannot mix"
    { "x": _, x: _ } => "cannot mix"
    { "x": _, .. } => "map always open"
    { .., x: 1 } => "record"
    { .., "x": 1 } => "map"
    { -true: _, x: 1 } => "map with error"
    { .. } => "treated as record"
  }
}
