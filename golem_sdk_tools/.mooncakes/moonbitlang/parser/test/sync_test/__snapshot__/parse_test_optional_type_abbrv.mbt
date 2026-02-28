fn optional(x : Int?) -> Int? {}

fn complex_optional(x : Int?, y : (String?, Bool)?) -> Array[Int]? {}

fn complex_optional2(x : Array[(String?, Bool)?]?) -> Unit {}

let a : Int? = Some(1)

let b : String? = Some("test")

let c : @map.Map[Int?,String?]? = Some(@map.of([(a,b)])) 

let d : Array[@map.Map[Int?,String?]?]? = Some([c]) 

let e : Array[(Int?, String)?] = [Some((Some(1), "test"))] 

let f : ((Int??, Bool) -> Int?)? = None




