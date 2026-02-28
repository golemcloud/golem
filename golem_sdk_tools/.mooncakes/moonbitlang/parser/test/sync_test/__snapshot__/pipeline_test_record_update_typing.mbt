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

fn init {
  let ra = { r1:893, r2:false, r3:46.49 }
  let rb_infer = {..ra, r1:894, }
  let rb_check : R = {..rb_infer, r3:89.4, }
  let nr = { nr1:16, nr2:{..rb_check, r2:true, } }
  let nr_check : Nested_r = { nr1:16, nr2:{..rb_check, r2:true, } }
  ()
}

