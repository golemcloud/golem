//!build: -w -27
fn init {
  let json1 : Json = { "x": [42, true, Json::null(), { "y": "z" }] }
  println(json1)
  let json2 : Json = { "x": false }
  println(json2)
  let x : Double = 42
  let json2 : Json = [json1, Json::number(x)]
  println(json2.stringify())
  let jsons = [ json1, false, json2 ]
  let json3 : Json = [ "begin", ..jsons, "end" ]
  println(json3.stringify())
  let json4  : Json =
    #| multi
    #| line
    #| json
    #| literal
  println(json4.stringify())

  // let json5 = Json::{ "x": 42, "y": "z" }  
  // type Json is not a record type
  // let json6 : Json = {
  //   #| multi    
  //   : #| line
  // }
  // println(json6.stringify())
  let json7 : Json = {
    "multi": #| line
             #| next
    ,
    "line": #| multi
            #| next
    ,
  }
  println(json7.stringify())
  f(5, "world")
}

// string interpolation also works
fn f(x : Int, y : String) -> Unit {
  let json1 : Json = "Hello, \{x} \{y}"
  println(json1)
  let json2 : Json =
    $| {
    $| "x": \{x},
    $| "y": \{y}
    $| }
    $|
  println(json2)
}
