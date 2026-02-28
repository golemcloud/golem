///|
#alias(VT)
struct T(VT)

///|
pub fn f(x : T) -> Unit {
  ignore(x)
}
//--------------------------------

///|
type PB = P[PA]

///|
struct PA(P[PB])
// effectively,
// struct PA(P[P[PA]])

///|
struct P[X](X)

///|
pub fn g(x : PA) -> Unit {
  ignore(x)
}
//--------------------------------

///|
#alias(FA)
struct F((Int) -> FA)

///|
pub fn h(x : F) -> Unit {
  ignore(x)
}

//--------------------------------

///|
#alias(GG2)
struct G2((G1, Int))

///|
struct G1((GG2, Int))

///|
pub fn p(x : G2) -> Unit {
  ignore(x)
}
