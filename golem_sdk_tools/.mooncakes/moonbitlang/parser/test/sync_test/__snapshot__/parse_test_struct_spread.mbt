struct Imag {
  real: Int
  imag: Int
}

fn init {
  let i = {real: 1, imag: 1,}
  let j = {..i,}
  let k = {..i}
  let l = {..i, real: 1}
  let real = 1
  let m = {..i, real}
}
