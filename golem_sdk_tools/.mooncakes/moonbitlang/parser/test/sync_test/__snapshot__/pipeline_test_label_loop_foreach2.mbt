suberror Err {
  Err(Char)
}

///|
fn[A : Show, B : Show] f(
  xs : Array[A],
  ys : Array[Int],
  zs : Array[B],
  ws : Array[Char]
) -> Unit raise Err {
  let buf = @moonbitlang/core/buffer.new()
  let mut first = true
  l1~: for i = 0; i < xs.length(); i = i + 1 {
    buf.write_string(xs[i].to_string())
    l2~: for y in ys.iter() {
      buf.write_string(y.to_string())
      l3~: for j = 0; j < zs.length(); j = j + 1 {
        buf.write_string(zs[j].to_string())
        l4~:for w in ws.iter() {
          match w {
            '!' => { continue l4~ }
            '@' => { break l1~ }
            '#' => { break l2~ }
            '$' => { break l3~ }
            '%' => {
              if first {
                first = false
                continue l1~ 2
              }
            }
            '^' => {
              if first {
                first = false
                continue l2~
              }
            }
            '&' => {
              if first {
                first = false
                continue l3~ 2
              }
            }
            '*' => {
              buf.write_string(w.to_string())
              break l4~
            }
            _ => {
              raise Err(w)
            }
          }
        }
      }
    }
  }
  println(buf)
}

///|

fn main {
  try {
    f(["x", "y"], [1, 2], ['a', 'b'], ['!'])
    f(["x", "y"], [1, 2], ['a', 'b'], ['@'])
    f(["x", "y"], [1, 2], ['a', 'b'], ['#'])
    f(["x", "y"], [1, 2], ['a', 'b'], ['$'])
    f(["x", "y", "z"], [1, 2], ['a', 'b'], ['%'])
    f(["x", "y"], [1, 2], ['a', 'b'], ['^'])
    f(["x", "y"], [1, 2], ['a', 'b', 'c'], ['&'])
    f(["x"], [1], ['a'], ['*', '1', '2'])
    f(["x"], [1], ['a'], ['('])
  } catch {
    Err(c) => { 
      println("unknown character: ")
      println(c) 
    }
  }
}
