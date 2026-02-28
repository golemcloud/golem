fn main {
  // This may be a bug from upstream  
  @moonbitlang/core/builtin.Map::from_array([(1,"hel")]) |> println
  { 1 :  "hel"} |> println
}