struct MyType(Int)

impl ToJson for MyType with to_json(self) {
  { "MyType": self.0 }
}

fn init {
  let x = 42
  let y = "def"
  let z = [ 1, 2, 3 ]
  let w = MyType(24)
  let json_arr : Array[Json] = [ 1, "2" ]
  let json : Json = [ x, y, z, [ ..json_arr, w ] ]
  println(json.stringify())
  println(H::B("x",3, A(2)).to_json().stringify())
  let arr_of_json : Array[Json] = [ x, y ]
  let arr_of_json2 : ArrayView[Json] = [ x, y ]
  let arr_of_json3 : FixedArray[Json] = [ x, y ]
  println(Json::array(arr_of_json).stringify())
  println(Json::array(arr_of_json2.to_array()).stringify())
  println(Json::array(arr_of_json3[:].to_array()).stringify())
  let map_of_json : Map[String, Json] = { "x": x, "y": y }
  println(Json::object(map_of_json).stringify())
}


enum H {
  A(Int)
  B(String, Int,H)
}

impl ToJson for H with to_json(self) {
  match self {
    A(x) => ["A", x]
    B(x, y, z) => ["B", x, y, z]
  }
}



