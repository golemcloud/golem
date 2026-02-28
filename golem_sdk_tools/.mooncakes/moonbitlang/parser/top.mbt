///|
using @tokens {type Comment}

///|
using @list {type List}

///|
using @basic {type Position, type Report, type Location}

///|
pub(all) enum Parser {
  MoonYacc
  Handrolled
  // Mixed
}

///|
fn attach_docstrings(
  docstrings : Array[List[(Location, Comment)]],
  toplevels : @syntax.Impls,
) -> Unit {

  // skip the docstring before the posisiton, return the last skipped docstring.
  fn skip_docstrings_before(pos : Position) -> List[(Location, Comment)]? {
    let mut previous = None
    while docstrings.last() is Some(comments) &&
          comments.last().unwrap().0.end <= pos {
      previous = docstrings.pop()
    }
    previous
  }

  fn make_doc(
    comments : List[(@basic.Location, @tokens.Comment)],
  ) -> @syntax.DocString {
    {
      content: comments.map(p => {
        match p.1.content {
          [.. "///|", .. remain] =>
            if remain.is_blank() {
              ""
            } else {
              remain.to_string()
            }
          [.. "///", .. remain] => remain.to_string()
          _ => panic()
        }
      }),
      loc: {
        start: comments.head().unwrap().0.start,
        end: comments.last().unwrap().0.end,
      },
    }
  }

  for toplevel in toplevels {
    let previous = skip_docstrings_before(toplevel.loc().start)
    let doc = previous.map(make_doc).unwrap_or(@syntax.DocString::empty())
    match toplevel {
      TopTypeDef(td) => {
        td.doc = doc
        match td.components {
          // there is no docstring inside the types
          Abstract
          | Extern
          | Newtype(_)
          | Alias(_)
          | Error(NoPayload | SinglePayload(_))
          | TupleStruct(_) => ()
          // handle docstring before the enum/suberror constructor and struct fields 
          Error(EnumPayload(constrs)) | Variant(constrs) =>
            constrs.each(constr => {
              let previous = skip_docstrings_before(constr.loc.start)
                .map(make_doc)
                .unwrap_or(@syntax.DocString::empty())
              constr.doc = previous
            })
          Record(fields) =>
            fields.each(field => {
              let previous = skip_docstrings_before(field.loc.start)
                .map(make_doc)
                .unwrap_or(@syntax.DocString::empty())
              field.doc = previous
            })
        }
      }
      TopFuncDef(fun_decl~, ..) => fun_decl.doc = doc
      TopFuncAlias(..) as fa => fa.doc = doc
      TopLetDef(..) as ld => ld.doc = doc
      TopExpr(..) => ()
      TopImplRelation(..) as imp => imp.doc = doc
      TopTest(..) as test_ => test_.doc = doc
      TopTrait(decl) => decl.doc = doc
      TopBatchTypeAlias(..) as decl => decl.doc = doc
      TopBatchTraitAlias(..) as decl => decl.doc = doc
      TopView(..) as view => view.doc = doc
      TopImpl(..) as imp => imp.doc = doc
      TopUsing(..) as using_stmt => using_stmt.doc = doc
    }
    skip_docstrings_before(toplevel.loc().end) |> ignore
  }
}

///|
pub fn parse_string(
  source : String,
  name? : String = "",
  parser? : Parser = Handrolled,
) -> (@syntax.Impls, Array[Report]) {
  let { tokens, docstrings, .. } = @lexer.tokens_from_string(
    source,
    comment=true,
    name~,
  )
  fn parse_by_moonyacc(tokens : @tokens.Triples) {
    let tokens = tokens.filter(fn(triple) {
      !(triple.0 is @tokens.Token::NEWLINE ||
      triple.0 is @tokens.Token::COMMENT(_))
    })
    (@yacc_parser.structure(tokens), []) catch {
      UnexpectedEndOfInput(pos, _) =>
        (
          @list.empty(),
          [
            Report::{
              loc: { start: pos, end: pos },
              msg: "Unexpected end of file.",
            },
          ],
        )
      UnexpectedToken(token, (start, end), _) =>
        (
          @list.empty(),
          [Report::{ loc: { start, end }, msg: "Unexpected `\{token}`." }],
        )
    }
  }

  let (impls, diagnostics) = match parser {
    MoonYacc => parse_by_moonyacc(tokens)
    Handrolled => @handrolled_parser.parse(tokens)
  }
  attach_docstrings(docstrings, impls)
  (impls, diagnostics)
}

///|
pub fn parse_file(
  path : String,
  parser? : Parser = Handrolled,
) -> (@syntax.Impls, Array[Report]) raise @fs.IOError {
  let source = @fs.read_file_to_string(path)
  parse_string(source, name=path, parser~)
}
