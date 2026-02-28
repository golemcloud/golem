fn test_add(x : FixedArray[String])->String{
  let mut res = ""
  x.each(x => { res += x })
  res
}

fn init {
  test_add(["a","b"])|>println
}
