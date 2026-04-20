///|
/// Represents a parsed public function signature from a `.mbt` source file.
pub(all) struct FnSignature {
  gen_name : String
  user_name : String
  params : Array[String]
  return_type : String
}

///|
/// Parses a `.mbt` source file and extracts public function signatures.
///
/// Uses `@parser.parse_string` to parse the source into an AST, then walks the
/// AST to find `pub fn` declarations and extracts their signatures.
pub fn parse_mbt_source(content : String) -> Array[FnSignature] {
  let (impls, _reports) = @parser.parse_string(content)
  let fns : Array[FnSignature] = []
  impls.each(fn(impl_) {
    match impl_ {
      TopFuncDef(fun_decl~, ..) => {
        guard fun_decl.vis is Pub(..) else { return }
        let gen_name = fun_decl.name.name
        let params : Array[String] = []
        match fun_decl.decl_params {
          Some(ps) =>
            ps.each(fn(p) {
              match p {
                Positional(ty=Some(ty), ..)
                | DiscardPositional(ty=Some(ty), ..) =>
                  params.push(type_to_string(ty))
                _ => params.push("Unknown")
              }
            })
          None => ()
        }
        let return_type = match fun_decl.return_type {
          Some(ty) => type_to_string(ty)
          None => "Unit"
        }
        let user_name = match gen_name.strip_prefix("mbt_ffi_") {
          Some(stripped) => stripped.to_string()
          None => gen_name
        }
        fns.push({ gen_name, user_name, params, return_type })
      }
      _ => ()
    }
  })
  fns
}

///|
/// Converts a MoonBit syntax AST type to its string representation.
fn type_to_string(ty : @syntax.Type) -> String {
  match ty {
    Name(constr_id~, tys~, ..) => {
      let base = match constr_id.id {
        Ident(name~) => name
        Dot(pkg~, id~) => "@\{pkg}.\{id}"
      }
      if tys.is_empty() {
        base
      } else {
        let sb = StringBuilder::new()
        sb.write_string(base)
        sb.write_char('[')
        let mut first = true
        tys.each(fn(t) {
          if not(first) {
            sb.write_string(", ")
          }
          first = false
          sb.write_string(type_to_string(t))
        })
        sb.write_char(']')
        sb.to_string()
      }
    }
    Option(ty~, ..) => "\{type_to_string(ty)}?"
    Tuple(tys~, ..) => {
      let sb = StringBuilder::new()
      sb.write_char('(')
      let mut first = true
      tys.each(fn(t) {
        if not(first) {
          sb.write_string(", ")
        }
        first = false
        sb.write_string(type_to_string(t))
      })
      sb.write_char(')')
      sb.to_string()
    }
    _ => "Unknown"
  }
}
