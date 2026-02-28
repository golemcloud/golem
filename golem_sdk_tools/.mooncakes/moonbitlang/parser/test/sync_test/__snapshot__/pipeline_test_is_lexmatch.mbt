fn init {
  // Basic exact match tests
  println("abc" lexmatch? "abc")
  println("abcd" lexmatch? "abc")
  println("abc" lexmatch? ("abc", x) && x == "")
  println("abcd" lexmatch? ("abc", x) && x == "d")
  
  // Longest match tests
  println("abc" lexmatch? "abc" with longest)
  println("abcd" lexmatch? "abc" with longest)
  println("abc" lexmatch? ("abc", x) with longest && x == "")
  println("abcd" lexmatch? ("abc", x) with longest && x == "d")
  
  // Empty string tests
  println("" lexmatch? "")
  println("" lexmatch? ("", x) && x == "")
  println("abc" lexmatch? "")
  println("" lexmatch? "abc")
  
  // Partial match tests
  println("xyz" lexmatch? "abc")
  println("axyz" lexmatch? "a")
  println("axyz" lexmatch? ("a", rest) && rest == "xyz")
  
  // Multiple character matches
  println("hello world" lexmatch? "hello")
  println("hello world" lexmatch? ("hello", x) && x == " world")
  println("hello" lexmatch? "hello world")
  
  // Special characters and patterns
  println("123abc" lexmatch? "123")
  println("123abc" lexmatch? ("123", x) && x == "abc")
  println("!@#$%" lexmatch? "!@#")
  println("!@#$%" lexmatch? ("!@#", x) && x == "$%")
  
  // Unicode and international characters
  println("你好世界" lexmatch? "你好")
  println("你好世界" lexmatch? ("你好", x) && x == "世界")
  println("café" lexmatch? "caf")
  println("café" lexmatch? ("caf", x) && x == "é")
  
  // Longer strings
  println("abcdefghij" lexmatch? "abcde")
  println("abcdefghij" lexmatch? ("abcde", x) && x == "fghij")
  
  // Single character tests
  println("a" lexmatch? "a")
  println("a" lexmatch? ("a", x) && x == "")
  println("ab" lexmatch? ("a", x) && x == "b")
  
  // Whitespace tests
  println(" abc" lexmatch? " ")
  println(" abc" lexmatch? (" ", x) && x == "abc")
  println("abc " lexmatch? "abc")
  println("abc " lexmatch? ("abc", x) && x == " ")
  println("\tabc" lexmatch? "\t")
  println("\nabc" lexmatch? "\n")
  
  // Numbers and digits
  println("0123456789" lexmatch? "012345")
  println("0123456789" lexmatch? ("012345", x) && x == "6789")
  
  // Mixed alphanumeric
  println("abc123def456" lexmatch? "abc123")
  println("abc123def456" lexmatch? ("abc123", x) && x == "def456")
  
  // Case sensitivity
  println("ABC" lexmatch? "ABC")
  println("ABC" lexmatch? "abc")
  println("abc" lexmatch? "ABC")
  
  // Repeated characters
  println("aaa" lexmatch? "aa")
  println("aaa" lexmatch? ("aa", x) && x == "a")
  println("aaabbb" lexmatch? "aaa")
  println("aaabbb" lexmatch? ("aaa", x) && x == "bbb")
  
  // Longest match with multiple options
  println("test" lexmatch? "test" with longest)
  println("testing" lexmatch? "test" with longest)
  println("testing" lexmatch? ("test", x) with longest && x == "ing")
  
  // Edge cases with rest being empty vs non-empty
  println("match" lexmatch? ("match", x) && x == "")
  println("matched" lexmatch? ("match", x) && x == "ed")
  println("m" lexmatch? ("m", x) && x == "")

  // Case insensitive
  println("Abc" lexmatch? "(?i:ABC)")
  println("AbCdEf" lexmatch? "(?i:[a-f]+)")
  println("Abc" lexmatch? "(?i:ABC)" with longest)
  println("AbCdEf" lexmatch? "(?i:[a-f]+)" with longest)

  if "hello1234abc" lexmatch? ("hello" "[0-9]+", rest) {
    println(rest)
  }
}
