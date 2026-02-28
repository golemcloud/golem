fnalias @pkg.f
pub fnalias @pkg.(f, g, h)

fnalias local as alias
fnalias (
  local1 as alias1,
  local2 as alias2
)

fnalias LocalTypeName::meth1
fnalias LocalTypeName::meth1 as meth_two
fnalias LocalTypeName::(
  meth1,
  meth2 as meth_two
)

fnalias @pkg.Type::meth1
fnalias @pkg.Type::meth2 as meth_two
fnalias @pkg.Type::(
  meth1,
  meth2 as meth_two
)
