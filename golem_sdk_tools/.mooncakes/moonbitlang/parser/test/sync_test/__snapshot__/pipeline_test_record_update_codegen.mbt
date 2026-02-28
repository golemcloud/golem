struct Poly_r[T, K, L] {
  pr1:T
  pr2:K
  pr3:L
}

struct R {
  r1:Int
  r2:Bool
  r3:Double
}

struct Nested_r {
  nr1:Int
  nr2:R
}

fn[T, K, L] update_poly_r_snd_check (pr: Poly_r[T, K, L], pr2: K) ->
     Poly_r[T, K, L] {
  let ret : Poly_r[T, K, L] = {..pr, pr2, }
  ret
}

fn[T, K, L] update_poly_r_snd_infer (pr: Poly_r[T, K, L], pr2: K) ->
     Poly_r[T, K, L] {
  {..pr, pr2, }
}

let test1:(Bool) -> Unit = fn(b) {
  if b { println("ok\n") } else { println("not ok\n") }
}

let side_eff : Ref[Int] = { val: 0 }
fn make() -> R {
  side_eff.val = side_eff.val + 1
  { r1:114514, r2:false, r3:19.19 }
}

fn init {
  let ra = { r1:893, r2:false, r3:46.49 }
  let rb_infer = {..ra, r1:894, }
  let rb_check : R = {..rb_infer, r1:895, }
  let nr = { nr1:16, nr2:{..rb_check, r2:true, } }
  let nr_check : Nested_r = { nr1:16, nr2:{..rb_check, r2:true, } }
  let rx = {..make(), r1:999, }
  let ry = {..nr_check.nr2, r1:666, }
  test1(side_eff.val == 1)
  test1(rb_infer.r1 == 894)
  test1(rb_check.r1 == 895)
  test1(nr.nr2.r2)
  test1(nr_check.nr2.r1 == 895)
}

