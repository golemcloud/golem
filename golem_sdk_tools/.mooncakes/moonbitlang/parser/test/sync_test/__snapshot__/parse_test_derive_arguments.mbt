struct T {
  x: Int
  y: Int
} derive(FromJson(i_am_empty_labelled_arg))

struct T2 {
  x: Int
  y: Int
} derive(FromJson(123)) // I am positional arg

struct T3 {
  x: Int
  y: Int
} derive(FromJson(i_am_labelled="arg"))

struct T4 {
  x: Int
  y: Int
} derive(FromJson(i_am_also(labelled, "arg")))

struct T5 {
  x: Int
  y: Int
} derive(FromJson(i_am="complex", but(this, "should", parse == true)))

struct TThatFailsParsing {
  x: Int
  y: Int
} derive(FromJson(00000imNotAnExpression))

struct TExotic {
  x: Int
  y: Int
} derive(FromJson((fn() {"Technically you can do this, it's just not very recommended :/"})))
