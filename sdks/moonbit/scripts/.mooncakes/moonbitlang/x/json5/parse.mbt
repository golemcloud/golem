// Copyright 2025 International Digital Economy Academy
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

///|
pub fn parse(input : String) -> Json raise ParseError {
  let ctx = ParseContext::make(input)
  let val = parse_value(ctx)
  lex_skip_whitespace(ctx)
  if ctx.offset >= ctx.end_offset {
    val
  } else {
    invalid_char(ctx)
  }
}

///|
fn parse_value(ctx : ParseContext) -> Json raise ParseError {
  let tok = lex_value(ctx)
  parse_value2(ctx, tok)
}

///|
fn parse_value2(ctx : ParseContext, tok : Token) -> Json raise ParseError {
  match tok {
    Null => Json::null()
    True => Json::boolean(true)
    False => Json::boolean(false)
    Number(n) => Json::number(n)
    String(s) => Json::string(s)
    LBrace => parse_object(ctx)
    LBracket => parse_array(ctx)
    _ => abort("unreachable")
  }
}

///|
fn parse_object(ctx : ParseContext) -> Json raise ParseError {
  let map = Map::new()
  for {
    let tok = lex_property_name(ctx)
    match tok {
      RBrace => break
      String(name) => {
        let tok2 = lex_after_property_name(ctx)
        match tok2 {
          Colon => {
            let val = parse_value(ctx)
            map.set(name, val)
            let tok3 = lex_after_object_value(ctx)
            match tok3 {
              Comma => continue
              RBrace => break
              _ => abort("unreachable")
            }
          }
          _ => abort("unreachable")
        }
      }
      _ => abort("unreachable")
    }
  }
  Json::object(map)
}

///|
fn parse_array(ctx : ParseContext) -> Json raise ParseError {
  let vec = []
  for {
    let tok = lex_value(ctx, allow_rbracket=true)
    match tok {
      RBracket => break
      _ => {
        vec.push(parse_value2(ctx, tok))
        let tok2 = lex_after_array_value(ctx)
        match tok2 {
          Comma => continue
          RBracket => break
          _ => abort("unreachable")
        }
      }
    }
  }
  Json::array(vec)
}
