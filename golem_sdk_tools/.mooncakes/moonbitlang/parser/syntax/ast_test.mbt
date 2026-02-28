///|
let dummy_loc : @basic.Location = {
  let pos = @basic.Position::{ fname: "", lnum: 0, bol: 0, cnum: 0 }
  { start: pos, end: pos }
}

///|
let null : Json = Json::null()

///|
let dummy_string_literal : @tokens.StringLiteral = "test_string"

///|
let dummy_byte_literal : @tokens.ByteLiteral = "b'A'"

///|
let dummy_bytes_literal : @tokens.BytesLiteral = "b\"ABC\""

///|
let dummy_char_literal : @tokens.CharLiteral = "'A'"

///|
let dummy_docstring : @syntax.DocString = {
  content: @list.from_array(["Test documentation"]),
  loc: dummy_loc,
}

///|
let dummy_attribute : @attribute.Attribute = {
  loc: dummy_loc,
  raw: "test_attr",
  parsed: None,
}

///|
test "visibility to_json" {
  json_inspect(@syntax.Default.json_repr(dummy_loc), content={
    "kind": "Visibility::Default",
    "loc": null,
    "children": {},
  })
  json_inspect(@syntax.Priv(loc=dummy_loc).json_repr(dummy_loc), content={
    "kind": "Visibility::Priv",
    "loc": null,
    "children": {},
  })
  json_inspect(
    @syntax.Pub(attr=Some("readonly"), loc=dummy_loc).json_repr(dummy_loc),
    content={
      "kind": "Visibility::Pub",
      "loc": null,
      "children": { "attr": "readonly" },
    },
  )
  json_inspect(@syntax.Pub(attr=None, loc=dummy_loc).json_repr(dummy_loc), content={
    "kind": "Visibility::Pub",
    "loc": null,
    "children": { "attr": null },
  })
}

///|
test "constant to_json" {
  json_inspect(@syntax.Bool(true).json_repr(dummy_loc), content={
    "kind": "Constant::Bool",
    "loc": null,
    "children": { "value": true },
  })
  json_inspect(@syntax.Byte(dummy_byte_literal).json_repr(dummy_loc), content={
    "kind": "Constant::Byte",
    "loc": null,
    "children": { "value": "b'A'" },
  })
  json_inspect(@syntax.Bytes(dummy_bytes_literal).json_repr(dummy_loc), content={
    "kind": "Constant::Bytes",
    "loc": null,
    "children": { "value": "b\"ABC\"" },
  })
  json_inspect(@syntax.Char(dummy_char_literal).json_repr(dummy_loc), content={
    "kind": "Constant::Char",
    "loc": null,
    "children": { "value": "'A'" },
  })
  json_inspect(@syntax.Int("42").json_repr(dummy_loc), content={
    "kind": "Constant::Int",
    "loc": null,
    "children": { "value": "42" },
  })
  json_inspect(@syntax.Int64("42L").json_repr(dummy_loc), content={
    "kind": "Constant::Int64",
    "loc": null,
    "children": { "value": "42L" },
  })
  json_inspect(@syntax.UInt("42U").json_repr(dummy_loc), content={
    "kind": "Constant::UInt",
    "loc": null,
    "children": { "value": "42U" },
  })
  json_inspect(@syntax.UInt64("42UL").json_repr(dummy_loc), content={
    "kind": "Constant::UInt64",
    "loc": null,
    "children": { "value": "42UL" },
  })
  json_inspect(@syntax.Float("3.14").json_repr(dummy_loc), content={
    "kind": "Constant::Float",
    "loc": null,
    "children": { "value": "3.14" },
  })
  json_inspect(@syntax.Double("3.14159").json_repr(dummy_loc), content={
    "kind": "Constant::Double",
    "loc": null,
    "children": { "value": "3.14159" },
  })
  json_inspect(
    @syntax.Constant::String(dummy_string_literal).json_repr(dummy_loc),
    content={
      "kind": "Constant::String",
      "loc": null,
      "children": { "value": "test_string" },
    },
  )
  json_inspect(
    @syntax.BigInt("123456789012345678901234567890").json_repr(dummy_loc),
    content={
      "kind": "Constant::BigInt",
      "loc": null,
      "children": { "value": "123456789012345678901234567890" },
    },
  )
}

///|
test "label to_json" {
  let label = @syntax.Label::{ name: "test_label", loc: dummy_loc }
  json_inspect(label.json_repr(), content={
    "kind": "Label",
    "loc": null,
    "children": { "name": "test_label" },
  })
}

///|
test "constr_name to_json" {
  let constr_name = @syntax.ConstrName::{
    name: "TestConstructor",
    loc: dummy_loc,
  }
  json_inspect(constr_name.json_repr(), content={
    "kind": "ConstrName",
    "loc": null,
    "children": { "name": "TestConstructor" },
  })
}

///|
test "long_ident to_json" {
  json_inspect(
    @syntax.LongIdent::Ident(name="test_ident").json_repr(dummy_loc),
    content={
      "kind": "LongIdent::Ident",
      "loc": null,
      "children": { "value": "test_ident" },
    },
  )
  json_inspect(
    @syntax.LongIdent::Dot(pkg="test_pkg", id="test_name").json_repr(dummy_loc),
    content={
      "kind": "LongIdent::Dot",
      "loc": null,
      "children": { "pkg": "test_pkg", "id": "test_name" },
    },
  )
}

///|
test "type_name to_json" {
  let type_name = @syntax.TypeName::{
    name: @syntax.LongIdent::Ident(name="TestType"),
    is_object: false,
    loc: dummy_loc,
  }
  json_inspect(type_name.json_repr(), content={
    "kind": "TypeName",
    "loc": null,
    "children": {
      "name": {
        "kind": "LongIdent::Ident",
        "loc": null,
        "children": { "value": "TestType" },
      },
      "is_object": false,
    },
  })
}

///|
test "constr_id to_json" {
  let constr_id = @syntax.ConstrId::{
    id: @syntax.Dot(pkg="test_pkg", id="TestConstructor"),
    loc: dummy_loc,
  }
  json_inspect(constr_id.json_repr(), content={
    "kind": "ConstrId",
    "loc": null,
    "children": {
      "id": {
        "kind": "LongIdent::Dot",
        "loc": null,
        "children": { "pkg": "test_pkg", "id": "TestConstructor" },
      },
    },
  })
}

///|
test "error_type to_json" {
  json_inspect(@syntax.NoErrorType.json_repr(dummy_loc), content={
    "kind": "ErrorType::NoErrorType",
    "loc": null,
    "children": {},
  })
  json_inspect(
    @syntax.ErrorType::DefaultErrorType(loc=dummy_loc).json_repr(dummy_loc),
    content={
      "kind": "ErrorType::DefaultErrorType",
      "loc": null,
      "children": {},
    },
  )
  let int_type = @syntax.Name(
    constr_id=@syntax.ConstrId::{
      id: @syntax.LongIdent::Ident(name="Int"),
      loc: dummy_loc,
    },
    tys=@list.from_array([]),
    loc=dummy_loc,
  )
  json_inspect(@syntax.ErrorType(ty=int_type).json_repr(dummy_loc), content={
    "kind": "ErrorType::ErrorType",
    "loc": null,
    "children": {
      "ty": {
        "kind": "Type::Name",
        "loc": null,
        "children": {
          "constr_id": {
            "kind": "ConstrId",
            "loc": null,
            "children": {
              "id": {
                "kind": "LongIdent::Ident",
                "loc": null,
                "children": { "value": "Int" },
              },
            },
          },
          "tys": { "kind": "Type::Name::TypeList", "loc": null, "children": [] },
        },
      },
    },
  })
  json_inspect(@syntax.MaybeError(ty=int_type).json_repr(dummy_loc), content={
    "kind": "ErrorType::MaybeError",
    "loc": null,
    "children": {
      "ty": {
        "kind": "Type::Name",
        "loc": null,
        "children": {
          "constr_id": {
            "kind": "ConstrId",
            "loc": null,
            "children": {
              "id": {
                "kind": "LongIdent::Ident",
                "loc": null,
                "children": { "value": "Int" },
              },
            },
          },
          "tys": { "kind": "Type::Name::TypeList", "loc": null, "children": [] },
        },
      },
    },
  })
}

///|
test "type to_json" {
  let int_type = @syntax.Name(
    constr_id=@syntax.ConstrId::{
      id: @syntax.LongIdent::Ident(name="Int"),
      loc: dummy_loc,
    },
    tys=@list.from_array([]),
    loc=dummy_loc,
  )
  json_inspect(@syntax.Type::Any(loc=dummy_loc).json_repr(), content={
    "kind": "Type::Any",
    "loc": null,
    "children": {},
  })
  json_inspect(
    @syntax.Type::Arrow(
      args=@list.from_array([int_type]),
      res=int_type,
      err=@syntax.NoErrorType,
      is_async=None,
      loc=dummy_loc,
    ).json_repr(),
    content={
      "kind": "Type::Arrow",
      "loc": null,
      "children": {
        "args": {
          "kind": "Type::Arrow::ArgList",
          "loc": null,
          "children": [
            {
              "kind": "Type::Name",
              "loc": null,
              "children": {
                "constr_id": {
                  "kind": "ConstrId",
                  "loc": null,
                  "children": {
                    "id": {
                      "kind": "LongIdent::Ident",
                      "loc": null,
                      "children": { "value": "Int" },
                    },
                  },
                },
                "tys": {
                  "kind": "Type::Name::TypeList",
                  "loc": null,
                  "children": [],
                },
              },
            },
          ],
        },
        "res": {
          "kind": "Type::Name",
          "loc": null,
          "children": {
            "constr_id": {
              "kind": "ConstrId",
              "loc": null,
              "children": {
                "id": {
                  "kind": "LongIdent::Ident",
                  "loc": null,
                  "children": { "value": "Int" },
                },
              },
            },
            "tys": {
              "kind": "Type::Name::TypeList",
              "loc": null,
              "children": [],
            },
          },
        },
        "err": { "kind": "ErrorType::NoErrorType", "loc": null, "children": {} },
        "is_async": null,
      },
    },
  )
  json_inspect(
    @syntax.Type::Tuple(tys=@list.from_array([int_type]), loc=dummy_loc).json_repr(),
    content={
      "kind": "Type::Tuple",
      "loc": null,
      "children": {
        "tys": {
          "kind": "Type::Tuple::TypeList",
          "loc": null,
          "children": [
            {
              "kind": "Type::Name",
              "loc": null,
              "children": {
                "constr_id": {
                  "kind": "ConstrId",
                  "loc": null,
                  "children": {
                    "id": {
                      "kind": "LongIdent::Ident",
                      "loc": null,
                      "children": { "value": "Int" },
                    },
                  },
                },
                "tys": {
                  "kind": "Type::Name::TypeList",
                  "loc": null,
                  "children": [],
                },
              },
            },
          ],
        },
      },
    },
  )
  json_inspect(
    @syntax.Type::Option(ty=int_type, loc=dummy_loc, question_loc=dummy_loc).json_repr(),
    content={
      "kind": "Type::Option",
      "loc": null,
      "children": {
        "ty": {
          "kind": "Type::Name",
          "loc": null,
          "children": {
            "constr_id": {
              "kind": "ConstrId",
              "loc": null,
              "children": {
                "id": {
                  "kind": "LongIdent::Ident",
                  "loc": null,
                  "children": { "value": "Int" },
                },
              },
            },
            "tys": {
              "kind": "Type::Name::TypeList",
              "loc": null,
              "children": [],
            },
          },
        },
      },
    },
  )
  json_inspect(
    @syntax.Type::Object(@syntax.ConstrId::{
      id: @syntax.LongIdent::Ident(name="TestObject"),
      loc: dummy_loc,
    }).json_repr(),
    content={
      "kind": "Type::Object",
      "loc": null,
      "children": {
        "value": {
          "kind": "ConstrId",
          "loc": null,
          "children": {
            "id": {
              "kind": "LongIdent::Ident",
              "loc": null,
              "children": { "value": "TestObject" },
            },
          },
        },
      },
    },
  )
}

///|
test "hole to_json" {
  json_inspect(@syntax.Synthesized.json_repr(dummy_loc), content={
    "kind": "Hole::Synthesized",
    "loc": null,
    "children": {},
  })
  json_inspect(@syntax.Incomplete.json_repr(dummy_loc), content={
    "kind": "Hole::Incomplete",
    "loc": null,
    "children": {},
  })
  json_inspect(@syntax.Todo.json_repr(dummy_loc), content={
    "kind": "Hole::Todo",
    "loc": null,
    "children": {},
  })
}

///|
test "argument_kind to_json" {
  let label = @syntax.Label::{ name: "test_label", loc: dummy_loc }
  json_inspect(@syntax.ArgumentKind::Positional.json_repr(dummy_loc), content={
    "kind": "ArgumentKind::Positional",
    "loc": null,
    "children": {},
  })
  json_inspect(@syntax.ArgumentKind::Labelled(label).json_repr(dummy_loc), content={
    "kind": "ArgumentKind::Labelled",
    "loc": null,
    "children": {
      "value": {
        "kind": "Label",
        "loc": null,
        "children": { "name": "test_label" },
      },
    },
  })
  json_inspect(@syntax.ArgumentKind::LabelledPun(label).json_repr(dummy_loc), content={
    "kind": "ArgumentKind::LabelledPun",
    "loc": null,
    "children": {
      "value": {
        "kind": "Label",
        "loc": null,
        "children": { "name": "test_label" },
      },
    },
  })
  json_inspect(
    @syntax.ArgumentKind::LabelledOption(label~, question_loc=dummy_loc).json_repr(
      dummy_loc,
    ),
    content={
      "kind": "ArgumentKind::LabelledOption",
      "loc": null,
      "children": {
        "label": {
          "kind": "Label",
          "loc": null,
          "children": { "name": "test_label" },
        },
      },
    },
  )
  json_inspect(
    @syntax.ArgumentKind::LabelledOptionPun(label~, question_loc=dummy_loc).json_repr(
      dummy_loc,
    ),
    content={
      "kind": "ArgumentKind::LabelledOptionPun",
      "loc": null,
      "children": {
        "label": {
          "kind": "Label",
          "loc": null,
          "children": { "name": "test_label" },
        },
      },
    },
  )
}

///|
test "fn_kind to_json" {
  json_inspect(@syntax.FnKind::Lambda.json_repr(dummy_loc), content={
    "kind": "FnKind::Lambda",
    "loc": null,
    "children": {},
  })
  json_inspect(@syntax.FnKind::Arrow.json_repr(dummy_loc), content={
    "kind": "FnKind::Arrow",
    "loc": null,
    "children": {},
  })
}

///|
test "group to_json" {
  json_inspect(@syntax.Group::Brace.json_repr(dummy_loc), content={
    "kind": "Group::Brace",
    "loc": null,
    "children": {},
  })
  json_inspect(@syntax.Group::Paren.json_repr(dummy_loc), content={
    "kind": "Group::Paren",
    "loc": null,
    "children": {},
  })
}

///|
test "trailing_mark to_json" {
  json_inspect(@syntax.TrailingMark::Comma.json_repr(dummy_loc), content={
    "kind": "Trailing::Comma",
    "loc": null,
    "children": {},
  })
  json_inspect(@syntax.TrailingMark::Semi.json_repr(dummy_loc), content={
    "kind": "Trailing::Semi",
    "loc": null,
    "children": {},
  })
  json_inspect(@syntax.TrailingMark::None.json_repr(dummy_loc), content={
    "kind": "Trailing::None",
    "loc": null,
    "children": {},
  })
}

///|
test "apply_attr to_json" {
  json_inspect(@syntax.NoAttr.json_repr(dummy_loc), content={
    "kind": "ApplyAttr::NoAttr",
    "loc": null,
    "children": {},
  })
  json_inspect(@syntax.ApplyAttr::Exclamation.json_repr(dummy_loc), content={
    "kind": "ApplyAttr::Exclamation",
    "loc": null,
    "children": {},
  })
  json_inspect(@syntax.ApplyAttr::Question.json_repr(dummy_loc), content={
    "kind": "ApplyAttr::Question",
    "loc": null,
    "children": {},
  })
}

///|
test "TryOperatorKind to_json" {
  json_inspect(@syntax.TryOperatorKind::Question.json_repr(dummy_loc), content={
    "kind": "TryOperatorKind::Question",
    "loc": null,
    "children": {},
  })
  json_inspect(@syntax.TryOperatorKind::Exclamation.json_repr(dummy_loc), content={
    "kind": "TryOperatorKind::Exclamation",
    "loc": null,
    "children": {},
  })
}

///|
test "binder to_json" {
  let binder = @syntax.Binder::{ name: "test_binder", loc: dummy_loc }
  json_inspect(binder.json_repr(), content={
    "kind": "Binder",
    "loc": null,
    "children": { "name": "test_binder" },
  })
}

///|
test "var to_json" {
  let variable = @syntax.Var::{
    name: @syntax.LongIdent::Ident(name="test_var"),
    loc: dummy_loc,
  }
  json_inspect(variable.json_repr(), content={
    "kind": "Var",
    "loc": null,
    "children": {
      "name": {
        "kind": "LongIdent::Ident",
        "loc": null,
        "children": { "value": "test_var" },
      },
    },
  })
}

///|
test "constructor_extra_info to_json" {
  let type_name = @syntax.TypeName::{
    name: @syntax.LongIdent::Ident(name="TestType"),
    is_object: false,
    loc: dummy_loc,
  }
  json_inspect(@syntax.NoExtraInfo.json_repr(dummy_loc), content={
    "kind": "ConstructorExtraInfo::NoExtraInfo",
    "loc": null,
    "children": {},
  })
  json_inspect(@syntax.TypeName(type_name).json_repr(dummy_loc), content={
    "kind": "ConstructorExtraInfo::TypeName",
    "loc": null,
    "children": {
      "value": {
        "kind": "TypeName",
        "loc": null,
        "children": {
          "name": {
            "kind": "LongIdent::Ident",
            "loc": null,
            "children": { "value": "TestType" },
          },
          "is_object": false,
        },
      },
    },
  })
  json_inspect(@syntax.Package("test_package").json_repr(dummy_loc), content={
    "kind": "ConstructorExtraInfo::Package",
    "loc": null,
    "children": { "value": "test_package" },
  })
}

///|
test "constructor to_json" {
  let constr_name = @syntax.ConstrName::{
    name: "TestConstructor",
    loc: dummy_loc,
  }
  let constr = @syntax.Constructor::{
    name: constr_name,
    extra_info: @syntax.NoExtraInfo,
    loc: dummy_loc,
  }
  json_inspect(constr.json_repr(), content={
    "kind": "Constructor",
    "loc": null,
    "children": {
      "name": {
        "kind": "ConstrName",
        "loc": null,
        "children": { "name": "TestConstructor" },
      },
      "extra_info": {
        "kind": "ConstructorExtraInfo::NoExtraInfo",
        "loc": null,
        "children": {},
      },
    },
  })
}

///|
test "accessor to_json" {
  let label = @syntax.Label::{ name: "test_field", loc: dummy_loc }
  json_inspect(@syntax.Accessor::Label(label).json_repr(), content={
    "kind": "Accessor::Label",
    "loc": null,
    "children": {
      "value": {
        "kind": "Label",
        "loc": null,
        "children": { "name": "test_field" },
      },
    },
  })
  json_inspect(
    @syntax.Accessor::Index(tuple_index=0, loc=dummy_loc).json_repr(),
    content={
      "kind": "Accessor::Index",
      "loc": null,
      "children": { "tuple_index": 0 },
    },
  )
  json_inspect(@syntax.Accessor::Newtype(loc=dummy_loc).json_repr(), content={
    "kind": "Accessor::Newtype",
    "loc": null,
    "children": {},
  })
}

///|
test "exception_decl to_json" {
  json_inspect(@syntax.NoPayload.json_repr(dummy_loc), content={
    "kind": "ExceptionDecl::NoPayload",
    "loc": null,
    "children": {},
  })
  let int_type = @syntax.Name(
    constr_id=@syntax.ConstrId::{
      id: @syntax.LongIdent::Ident(name="Int"),
      loc: dummy_loc,
    },
    tys=@list.from_array([]),
    loc=dummy_loc,
  )
  json_inspect(@syntax.SinglePayload(int_type).json_repr(dummy_loc), content={
    "kind": "ExceptionDecl::SinglePayload",
    "loc": null,
    "children": {
      "value": {
        "kind": "Type::Name",
        "loc": null,
        "children": {
          "constr_id": {
            "kind": "ConstrId",
            "loc": null,
            "children": {
              "id": {
                "kind": "LongIdent::Ident",
                "loc": null,
                "children": { "value": "Int" },
              },
            },
          },
          "tys": { "kind": "Type::Name::TypeList", "loc": null, "children": [] },
        },
      },
    },
  })
  let constr_name = @syntax.ConstrName::{ name: "TestConstr", loc: dummy_loc }
  let constr_decl = @syntax.ConstrDecl::{
    name: constr_name,
    args: None,
    tag: None,
    loc: dummy_loc,
    attrs: @list.empty(),
    doc: dummy_docstring,
  }
  json_inspect(
    @syntax.EnumPayload(@list.from_array([constr_decl])).json_repr(dummy_loc),
    content={
      "kind": "ExceptionDecl::EnumPayload",
      "loc": null,
      "children": {
        "value": {
          "kind": "ExceptionDecl::EnumPayload::ConstrList",
          "loc": null,
          "children": [
            {
              "kind": "ConstrDecl",
              "loc": null,
              "children": {
                "name": {
                  "kind": "ConstrName",
                  "loc": null,
                  "children": { "name": "TestConstr" },
                },
                "args": null,
                "tag": null,
                "attrs": {
                  "kind": "ConstrDecl::AttrList",
                  "loc": null,
                  "children": [],
                },
                "doc": "Test documentation",
              },
            },
          ],
        },
      },
    },
  )
}

///|
test "type_desc to_json" {
  json_inspect(@syntax.Abstract.json_repr(dummy_loc), content={
    "kind": "TypeDesc::Abstract",
    "loc": null,
    "children": {},
  })
  json_inspect(@syntax.Extern.json_repr(dummy_loc), content={
    "kind": "TypeDesc::Extern",
    "loc": null,
    "children": {},
  })
  let int_type = @syntax.Name(
    constr_id=@syntax.ConstrId::{
      id: @syntax.LongIdent::Ident(name="Int"),
      loc: dummy_loc,
    },
    tys=@list.from_array([]),
    loc=dummy_loc,
  )
  json_inspect(@syntax.SinglePayload(int_type).json_repr(dummy_loc), content={
    "kind": "ExceptionDecl::SinglePayload",
    "loc": null,
    "children": {
      "value": {
        "kind": "Type::Name",
        "loc": null,
        "children": {
          "constr_id": {
            "kind": "ConstrId",
            "loc": null,
            "children": {
              "id": {
                "kind": "LongIdent::Ident",
                "loc": null,
                "children": { "value": "Int" },
              },
            },
          },
          "tys": { "kind": "Type::Name::TypeList", "loc": null, "children": [] },
        },
      },
    },
  })
  json_inspect(@syntax.Error(@syntax.NoPayload).json_repr(dummy_loc), content={
    "kind": "TypeDesc::Error",
    "loc": null,
    "children": {
      "value": {
        "kind": "ExceptionDecl::NoPayload",
        "loc": null,
        "children": {},
      },
    },
  })
  json_inspect(@syntax.TypeDesc::Alias(int_type).json_repr(dummy_loc), content={
    "kind": "TypeDesc::Alias",
    "loc": null,
    "children": {
      "value": {
        "kind": "Type::Name",
        "loc": null,
        "children": {
          "constr_id": {
            "kind": "ConstrId",
            "loc": null,
            "children": {
              "id": {
                "kind": "LongIdent::Ident",
                "loc": null,
                "children": { "value": "Int" },
              },
            },
          },
          "tys": { "kind": "Type::Name::TypeList", "loc": null, "children": [] },
        },
      },
    },
  })
}

///|
test "dot_dot_binder to_json" {
  json_inspect(@syntax.Underscore.json_repr(dummy_loc), content={
    "kind": "DotDotBinder::Underscore",
    "loc": null,
    "children": {},
  })
  json_inspect(@syntax.NoBinder.json_repr(dummy_loc), content={
    "kind": "DotDotBinder::NoBinder",
    "loc": null,
    "children": {},
  })
  let binder = @syntax.Binder::{ name: "test_binder", loc: dummy_loc }
  json_inspect(@syntax.BinderAs(binder).json_repr(dummy_loc), content={
    "kind": "DotDotBinder::BinderAs",
    "loc": null,
    "children": {
      "value": {
        "kind": "Binder",
        "loc": null,
        "children": { "name": "test_binder" },
      },
    },
  })
  json_inspect(@syntax.DotDotBinder::Binder(binder).json_repr(dummy_loc), content={
    "kind": "DotDotBinder::Binder",
    "loc": null,
    "children": {
      "value": {
        "kind": "Binder",
        "loc": null,
        "children": { "name": "test_binder" },
      },
    },
  })
}

///|
test "array_pattern to_json" {
  let any_pattern = @syntax.Pattern::Any(loc=dummy_loc)
  json_inspect(@syntax.ArrayPattern::Pattern(any_pattern).json_repr(), content={
    "kind": "ArrayPattern::Pattern",
    "loc": null,
    "children": {
      "value": { "kind": "Pattern::Any", "loc": null, "children": {} },
    },
  })
  json_inspect(
    @syntax.StringSpread(str=dummy_string_literal, loc=dummy_loc).json_repr(),
    content={
      "kind": "ArrayPattern::StringSpread",
      "loc": null,
      "children": { "str": "test_string" },
    },
  )
  json_inspect(
    @syntax.BytesSpread(bytes=dummy_bytes_literal, loc=dummy_loc).json_repr(),
    content={
      "kind": "ArrayPattern::BytesSpread",
      "loc": null,
      "children": { "bytes": "b\"ABC\"" },
    },
  )
  let binder = @syntax.Binder::{ name: "test_binder", loc: dummy_loc }
  json_inspect(
    @syntax.ConstSpread(binder~, pkg=Some("test_pkg"), loc=dummy_loc).json_repr(),
    content={
      "kind": "ArrayPattern::ConstSpread",
      "loc": null,
      "children": {
        "binder": {
          "kind": "Binder",
          "loc": null,
          "children": { "name": "test_binder" },
        },
        "pkg": "test_pkg",
      },
    },
  )
  json_inspect(
    @syntax.ConstSpread(binder~, pkg=None, loc=dummy_loc).json_repr(),
    content={
      "kind": "ArrayPattern::ConstSpread",
      "loc": null,
      "children": {
        "binder": {
          "kind": "Binder",
          "loc": null,
          "children": { "name": "test_binder" },
        },
        "pkg": null,
      },
    },
  )
}

///|
test "array_patterns to_json" {
  let any_pattern = @syntax.ArrayPattern::Pattern(
    @syntax.Pattern::Any(loc=dummy_loc),
  )
  json_inspect(
    @syntax.Closed(@list.from_array([any_pattern])).json_repr(dummy_loc),
    content={
      "kind": "ArrayPatterns::Closed",
      "loc": null,
      "children": {
        "value": {
          "kind": "ArrayPatterns::Closed::PatternList",
          "loc": null,
          "children": [
            {
              "kind": "ArrayPattern::Pattern",
              "loc": null,
              "children": {
                "value": { "kind": "Pattern::Any", "loc": null, "children": {} },
              },
            },
          ],
        },
      },
    },
  )
  json_inspect(
    @syntax.Open(
      @list.from_array([any_pattern]),
      @list.from_array([any_pattern]),
      @syntax.Underscore,
    ).json_repr(dummy_loc),
    content={
      "kind": "ArrayPatterns::Open",
      "loc": null,
      "children": {
        "0": {
          "kind": "ArrayPatterns::Open::LPatternList",
          "loc": null,
          "children": [
            {
              "kind": "ArrayPattern::Pattern",
              "loc": null,
              "children": {
                "value": { "kind": "Pattern::Any", "loc": null, "children": {} },
              },
            },
          ],
        },
        "1": {
          "kind": "ArrayPatterns::Open::RPatternList",
          "loc": null,
          "children": [
            {
              "kind": "ArrayPattern::Pattern",
              "loc": null,
              "children": {
                "value": { "kind": "Pattern::Any", "loc": null, "children": {} },
              },
            },
          ],
        },
        "2": { "kind": "DotDotBinder::Underscore", "loc": null, "children": {} },
      },
    },
  )
}

///|
test "embedded_code to_json" {
  json_inspect(@syntax.CodeString(dummy_string_literal).json_repr(dummy_loc), content={
    "kind": "EmbeddedCode::CodeString",
    "loc": null,
    "children": { "value": "test_string" },
  })
  json_inspect(
    @syntax.CodeMultilineString(@list.from_array(["line1", "line2", "line3"])).json_repr(
      dummy_loc,
    ),
    content={
      "kind": "EmbeddedCode::CodeMultilineString",
      "loc": null,
      "children": {
        "value": {
          "kind": "EmbeddedCode::CodeMultilineString::LiteralList",
          "loc": null,
          "children": ["line1", "line2", "line3"],
        },
      },
    },
  )
}

///|
test "func_stubs to_json" {
  json_inspect(
    @syntax.Import(
      module_name=dummy_string_literal,
      func_name=dummy_string_literal,
      language=None,
    ).json_repr(dummy_loc),
    content={
      "kind": "FuncStubs::Import",
      "loc": null,
      "children": {
        "module_name": "test_string",
        "func_name": "test_string",
        "language": null,
      },
    },
  )
  json_inspect(
    @syntax.Embedded(
      language=Some(dummy_string_literal),
      code=@syntax.CodeString(dummy_string_literal),
    ).json_repr(dummy_loc),
    content={
      "kind": "FuncStubs::Embedded",
      "loc": null,
      "children": {
        "language": "test_string",
        "code": {
          "kind": "EmbeddedCode::CodeString",
          "loc": null,
          "children": { "value": "test_string" },
        },
      },
    },
  )
  json_inspect(
    @syntax.Embedded(
      language=None,
      code=@syntax.CodeString(dummy_string_literal),
    ).json_repr(dummy_loc),
    content={
      "kind": "FuncStubs::Embedded",
      "loc": null,
      "children": {
        "language": null,
        "code": {
          "kind": "EmbeddedCode::CodeString",
          "loc": null,
          "children": { "value": "test_string" },
        },
      },
    },
  )
}

///|
test "parameter to_json" {
  let binder = @syntax.Binder::{ name: "param", loc: dummy_loc }
  let int_type = @syntax.Name(
    constr_id=@syntax.ConstrId::{
      id: @syntax.LongIdent::Ident(name="Int"),
      loc: dummy_loc,
    },
    tys=@list.from_array([]),
    loc=dummy_loc,
  )
  let unit_expr = @syntax.Expr::Unit(loc=dummy_loc, faked=false)
  json_inspect(
    @syntax.Parameter::DiscardPositional(ty=Some(int_type), loc=dummy_loc).json_repr(),
    content={
      "kind": "Parameter::DiscardPositional",
      "loc": null,
      "children": {
        "ty": {
          "kind": "Type::Name",
          "loc": null,
          "children": {
            "constr_id": {
              "kind": "ConstrId",
              "loc": null,
              "children": {
                "id": {
                  "kind": "LongIdent::Ident",
                  "loc": null,
                  "children": { "value": "Int" },
                },
              },
            },
            "tys": {
              "kind": "Type::Name::TypeList",
              "loc": null,
              "children": [],
            },
          },
        },
      },
    },
  )
  json_inspect(
    @syntax.Parameter::DiscardPositional(ty=None, loc=dummy_loc).json_repr(),
    content={
      "kind": "Parameter::DiscardPositional",
      "loc": null,
      "children": { "ty": null },
    },
  )
  json_inspect(
    @syntax.Parameter::Positional(binder~, ty=Some(int_type)).json_repr(),
    content={
      "kind": "Parameter::Positional",
      "loc": null,
      "children": {
        "binder": {
          "kind": "Binder",
          "loc": null,
          "children": { "name": "param" },
        },
        "ty": {
          "kind": "Type::Name",
          "loc": null,
          "children": {
            "constr_id": {
              "kind": "ConstrId",
              "loc": null,
              "children": {
                "id": {
                  "kind": "LongIdent::Ident",
                  "loc": null,
                  "children": { "value": "Int" },
                },
              },
            },
            "tys": {
              "kind": "Type::Name::TypeList",
              "loc": null,
              "children": [],
            },
          },
        },
      },
    },
  )
  json_inspect(@syntax.Parameter::Positional(binder~, ty=None).json_repr(), content={
    "kind": "Parameter::Positional",
    "loc": null,
    "children": {
      "binder": {
        "kind": "Binder",
        "loc": null,
        "children": { "name": "param" },
      },
      "ty": null,
    },
  })
  json_inspect(
    @syntax.Parameter::Labelled(binder~, ty=Some(int_type)).json_repr(),
    content={
      "kind": "Parameter::Labelled",
      "loc": null,
      "children": {
        "binder": {
          "kind": "Binder",
          "loc": null,
          "children": { "name": "param" },
        },
        "ty": {
          "kind": "Type::Name",
          "loc": null,
          "children": {
            "constr_id": {
              "kind": "ConstrId",
              "loc": null,
              "children": {
                "id": {
                  "kind": "LongIdent::Ident",
                  "loc": null,
                  "children": { "value": "Int" },
                },
              },
            },
            "tys": {
              "kind": "Type::Name::TypeList",
              "loc": null,
              "children": [],
            },
          },
        },
      },
    },
  )
  json_inspect(
    @syntax.Parameter::Optional(binder~, default=unit_expr, ty=Some(int_type)).json_repr(),
    content={
      "kind": "Parameter::Optional",
      "loc": null,
      "children": {
        "binder": {
          "kind": "Binder",
          "loc": null,
          "children": { "name": "param" },
        },
        "default": {
          "kind": "Expr::Unit",
          "loc": null,
          "children": { "faked": false },
        },
        "ty": {
          "kind": "Type::Name",
          "loc": null,
          "children": {
            "constr_id": {
              "kind": "ConstrId",
              "loc": null,
              "children": {
                "id": {
                  "kind": "LongIdent::Ident",
                  "loc": null,
                  "children": { "value": "Int" },
                },
              },
            },
            "tys": {
              "kind": "Type::Name::TypeList",
              "loc": null,
              "children": [],
            },
          },
        },
      },
    },
  )
  json_inspect(
    @syntax.Parameter::QuestionOptional(binder~, ty=Some(int_type)).json_repr(),
    content={
      "kind": "Parameter::QuestionOptional",
      "loc": null,
      "children": {
        "binder": {
          "kind": "Binder",
          "loc": null,
          "children": { "name": "param" },
        },
        "ty": {
          "kind": "Type::Name",
          "loc": null,
          "children": {
            "constr_id": {
              "kind": "ConstrId",
              "loc": null,
              "children": {
                "id": {
                  "kind": "LongIdent::Ident",
                  "loc": null,
                  "children": { "value": "Int" },
                },
              },
            },
            "tys": {
              "kind": "Type::Name::TypeList",
              "loc": null,
              "children": [],
            },
          },
        },
      },
    },
  )
}

///|
test "interp_elem to_json" {
  let unit_expr = @syntax.Expr::Unit(loc=dummy_loc, faked=false)
  json_inspect(
    @syntax.InterpElem::Literal(repr=dummy_string_literal, loc=dummy_loc).json_repr(),
    content={
      "kind": "InterpElem::Literal",
      "loc": null,
      "children": { "repr": "test_string" },
    },
  )
  json_inspect(
    @syntax.InterpElem::Expr(expr=unit_expr, loc=dummy_loc).json_repr(),
    content={
      "kind": "InterpElem::Expr",
      "loc": null,
      "children": {
        "expr": {
          "kind": "Expr::Unit",
          "loc": null,
          "children": { "faked": false },
        },
      },
    },
  )
}

///|
test "multiline_string_elem to_json" {
  let interp_elem = @syntax.InterpElem::Literal(
    repr=dummy_string_literal,
    loc=dummy_loc,
  )
  json_inspect(
    @syntax.MultilineStringElem::String("test string").json_repr(dummy_loc),
    content={
      "kind": "MultilineString::String",
      "loc": null,
      "children": { "value": "test string" },
    },
  )
  json_inspect(
    @syntax.MultilineStringElem::Interp(@list.from_array([interp_elem])).json_repr(
      dummy_loc,
    ),
    content={
      "kind": "MultilineString::Interp",
      "loc": null,
      "children": {
        "value": {
          "kind": "MultilineString::Interp::ElemList",
          "loc": null,
          "children": [
            {
              "kind": "InterpElem::Literal",
              "loc": null,
              "children": { "repr": "test_string" },
            },
          ],
        },
      },
    },
  )
}

///|
test "spreadable_elem to_json" {
  let unit_expr = @syntax.Expr::Unit(loc=dummy_loc, faked=false)
  json_inspect(@syntax.SpreadableElem::Regular(unit_expr).json_repr(), content={
    "kind": "SpreadableElem::Regular",
    "loc": null,
    "children": {
      "value": {
        "kind": "Expr::Unit",
        "loc": null,
        "children": { "faked": false },
      },
    },
  })
  json_inspect(
    @syntax.SpreadableElem::Spread(expr=unit_expr, loc=dummy_loc).json_repr(),
    content={
      "kind": "SpreadableElem::Spread",
      "loc": null,
      "children": {
        "expr": {
          "kind": "Expr::Unit",
          "loc": null,
          "children": { "faked": false },
        },
      },
    },
  )
}

///|
test "map_expr_elem to_json" {
  let unit_expr = @syntax.Expr::Unit(loc=dummy_loc, faked=false)
  let int_constant = @syntax.Int("42")
  let map_elem = @syntax.MapExprElem::{
    key: int_constant,
    expr: unit_expr,
    key_loc: dummy_loc,
    loc: dummy_loc,
  }
  json_inspect(map_elem.json_repr(), content={
    "kind": "MapExprElem",
    "loc": null,
    "children": {
      "key": {
        "kind": "Constant::Int",
        "loc": null,
        "children": { "value": "42" },
      },
      "expr": {
        "kind": "Expr::Unit",
        "loc": null,
        "children": { "faked": false },
      },
    },
  })
}

///|
test "field_def to_json" {
  let label = @syntax.Label::{ name: "field_name", loc: dummy_loc }
  let unit_expr = @syntax.Expr::Unit(loc=dummy_loc, faked=false)
  let field_def = @syntax.FieldDef::{
    label,
    expr: unit_expr,
    is_pun: false,
    loc: dummy_loc,
  }
  json_inspect(field_def.json_repr(), content={
    "kind": "FieldDef",
    "loc": null,
    "children": {
      "label": {
        "kind": "Label",
        "loc": null,
        "children": { "name": "field_name" },
      },
      "expr": {
        "kind": "Expr::Unit",
        "loc": null,
        "children": { "faked": false },
      },
      "is_pun": false,
    },
  })
}

///|
test "argument to_json" {
  let unit_expr = @syntax.Expr::Unit(loc=dummy_loc, faked=false)
  let argument = @syntax.Argument::{
    value: unit_expr,
    kind: @syntax.ArgumentKind::Positional,
  }
  json_inspect(argument.json_repr(), content={
    "kind": "Argument",
    "loc": null,
    "children": {
      "value": {
        "kind": "Expr::Unit",
        "loc": null,
        "children": { "faked": false },
      },
      "kind": {
        "kind": "ArgumentKind::Positional",
        "loc": null,
        "children": {},
      },
    },
  })
}

///|
test "case to_json" {
  let any_pattern = @syntax.Pattern::Any(loc=dummy_loc)
  let unit_expr = @syntax.Expr::Unit(loc=dummy_loc, faked=false)
  let case_with_guard = @syntax.Case::{
    pattern: any_pattern,
    guard_: Some(unit_expr),
    body: unit_expr,
  }
  let case_without_guard = @syntax.Case::{
    pattern: any_pattern,
    guard_: None,
    body: unit_expr,
  }
  json_inspect(case_with_guard.json_repr(), content={
    "kind": "Case",
    "loc": null,
    "children": {
      "pattern": { "kind": "Pattern::Any", "loc": null, "children": {} },
      "guard": {
        "kind": "Expr::Unit",
        "loc": null,
        "children": { "faked": false },
      },
      "body": {
        "kind": "Expr::Unit",
        "loc": null,
        "children": { "faked": false },
      },
    },
  })
  json_inspect(case_without_guard.json_repr(), content={
    "kind": "Case",
    "loc": null,
    "children": {
      "pattern": { "kind": "Pattern::Any", "loc": null, "children": {} },
      "guard": null,
      "body": {
        "kind": "Expr::Unit",
        "loc": null,
        "children": { "faked": false },
      },
    },
  })
}

///|
test "static_assertion to_json" {
  let int_type = @syntax.Name(
    constr_id=@syntax.ConstrId::{
      id: @syntax.LongIdent::Ident(name="Int"),
      loc: dummy_loc,
    },
    tys=@list.from_array([]),
    loc=dummy_loc,
  )
  let assertion = @syntax.StaticAssertion::{
    ty: int_type,
    trait_: @syntax.LongIdent::Ident(name="Show"),
    loc: dummy_loc,
    msg: "Test assertion message",
  }
  json_inspect(assertion.json_repr(), content={
    "kind": "StaticAssertion",
    "loc": null,
    "children": {
      "ty": {
        "kind": "Type::Name",
        "loc": null,
        "children": {
          "constr_id": {
            "kind": "ConstrId",
            "loc": null,
            "children": {
              "id": {
                "kind": "LongIdent::Ident",
                "loc": null,
                "children": { "value": "Int" },
              },
            },
          },
          "tys": { "kind": "Type::Name::TypeList", "loc": null, "children": [] },
        },
      },
      "trait": {
        "kind": "LongIdent::Ident",
        "loc": null,
        "children": { "value": "Show" },
      },
      "msg": "Test assertion message",
    },
  })
}

///|
test "func to_json" {
  let binder = @syntax.Binder::{ name: "param", loc: dummy_loc }
  let unit_expr = @syntax.Expr::Unit(loc=dummy_loc, faked=false)
  let int_type = @syntax.Name(
    constr_id=@syntax.ConstrId::{
      id: @syntax.LongIdent::Ident(name="Int"),
      loc: dummy_loc,
    },
    tys=@list.from_array([]),
    loc=dummy_loc,
  )
  let param = @syntax.Parameter::Positional(binder~, ty=Some(int_type))
  // let any_pattern = @syntax.Pattern::Any(loc=dummy_loc)
  json_inspect(
    @syntax.Func::{
      parameters: @list.from_array([param]),
      params_loc: dummy_loc,
      body: unit_expr,
      return_type: Some(int_type),
      error_type: @syntax.NoErrorType,
      kind: @syntax.FnKind::Lambda,
      has_error: Some(dummy_loc),
      is_async: None,
      loc: dummy_loc,
    }.json_repr(),
    content={
      "kind": "Func::Lambda",
      "loc": null,
      "children": {
        "parameters": {
          "kind": "Func::Lambda::ParameterList",
          "loc": null,
          "children": [
            {
              "kind": "Parameter::Positional",
              "loc": null,
              "children": {
                "binder": {
                  "kind": "Binder",
                  "loc": null,
                  "children": { "name": "param" },
                },
                "ty": {
                  "kind": "Type::Name",
                  "loc": null,
                  "children": {
                    "constr_id": {
                      "kind": "ConstrId",
                      "loc": null,
                      "children": {
                        "id": {
                          "kind": "LongIdent::Ident",
                          "loc": null,
                          "children": { "value": "Int" },
                        },
                      },
                    },
                    "tys": {
                      "kind": "Type::Name::TypeList",
                      "loc": null,
                      "children": [],
                    },
                  },
                },
              },
            },
          ],
        },
        "body": {
          "kind": "Expr::Unit",
          "loc": null,
          "children": { "faked": false },
        },
        "return_type": {
          "kind": "Type::Name",
          "loc": null,
          "children": {
            "constr_id": {
              "kind": "ConstrId",
              "loc": null,
              "children": {
                "id": {
                  "kind": "LongIdent::Ident",
                  "loc": null,
                  "children": { "value": "Int" },
                },
              },
            },
            "tys": {
              "kind": "Type::Name::TypeList",
              "loc": null,
              "children": [],
            },
          },
        },
        "error_type": {
          "kind": "ErrorType::NoErrorType",
          "loc": null,
          "children": {},
        },
        "kind": { "kind": "FnKind::Lambda", "loc": null, "children": {} },
        "has_error": { "kind": "Raise", "loc": null, "children": {} },
        "is_async": null,
      },
    },
  )
}

///|
test "expr basic types to_json" {
  let unit_expr = @syntax.Expr::Unit(loc=dummy_loc, faked=false)
  let int_constant = @syntax.Int("42")
  let variable = @syntax.Var::{
    name: @syntax.LongIdent::Ident(name="test_var"),
    loc: dummy_loc,
  }
  let label = @syntax.Label::{ name: "test_label", loc: dummy_loc }
  let argument = @syntax.Argument::{
    value: unit_expr,
    kind: @syntax.Positional,
  }
  json_inspect(@syntax.Expr::Unit(loc=dummy_loc, faked=false).json_repr(), content={
    "kind": "Expr::Unit",
    "loc": null,
    "children": { "faked": false },
  })
  json_inspect(
    @syntax.Expr::Constant(c=int_constant, loc=dummy_loc).json_repr(),
    content={
      "kind": "Expr::Constant",
      "loc": null,
      "children": {
        "constant": {
          "kind": "Constant::Int",
          "loc": null,
          "children": { "value": "42" },
        },
      },
    },
  )
  json_inspect(@syntax.Expr::Ident(id=variable, loc=dummy_loc).json_repr(), content={
    "kind": "Expr::Ident",
    "loc": null,
    "children": {
      "id": {
        "kind": "Var",
        "loc": null,
        "children": {
          "name": {
            "kind": "LongIdent::Ident",
            "loc": null,
            "children": { "value": "test_var" },
          },
        },
      },
    },
  })
  json_inspect(
    @syntax.Expr::Apply(
      func=unit_expr,
      args=@list.from_array([argument]),
      attr=@syntax.NoAttr,
      loc=dummy_loc,
    ).json_repr(),
    content={
      "kind": "Expr::Apply",
      "loc": null,
      "children": {
        "func": {
          "kind": "Expr::Unit",
          "loc": null,
          "children": { "faked": false },
        },
        "args": {
          "kind": "Expr::Apply::ArgumentList",
          "loc": null,
          "children": [
            {
              "kind": "Argument",
              "loc": null,
              "children": {
                "value": {
                  "kind": "Expr::Unit",
                  "loc": null,
                  "children": { "faked": false },
                },
                "kind": {
                  "kind": "ArgumentKind::Positional",
                  "loc": null,
                  "children": {},
                },
              },
            },
          ],
        },
        "attr": { "kind": "ApplyAttr::NoAttr", "loc": null, "children": {} },
      },
    },
  )
  json_inspect(
    @syntax.Infix(op=variable, lhs=unit_expr, rhs=unit_expr, loc=dummy_loc).json_repr(),
    content={
      "kind": "Expr::Infix",
      "loc": null,
      "children": {
        "op": {
          "kind": "Var",
          "loc": null,
          "children": {
            "name": {
              "kind": "LongIdent::Ident",
              "loc": null,
              "children": { "value": "test_var" },
            },
          },
        },
        "lhs": {
          "kind": "Expr::Unit",
          "loc": null,
          "children": { "faked": false },
        },
        "rhs": {
          "kind": "Expr::Unit",
          "loc": null,
          "children": { "faked": false },
        },
      },
    },
  )
  json_inspect(
    @syntax.Unary(op=variable, expr=unit_expr, loc=dummy_loc).json_repr(),
    content={
      "kind": "Expr::Unary",
      "loc": null,
      "children": {
        "op": {
          "kind": "Var",
          "loc": null,
          "children": {
            "name": {
              "kind": "LongIdent::Ident",
              "loc": null,
              "children": { "value": "test_var" },
            },
          },
        },
        "expr": {
          "kind": "Expr::Unit",
          "loc": null,
          "children": { "faked": false },
        },
      },
    },
  )
  json_inspect(
    @syntax.Expr::Array(exprs=@list.from_array([unit_expr]), loc=dummy_loc).json_repr(),
    content={
      "kind": "Expr::Array",
      "loc": null,
      "children": {
        "exprs": {
          "kind": "Expr::Array::ExprList",
          "loc": null,
          "children": [
            {
              "kind": "Expr::Unit",
              "loc": null,
              "children": { "faked": false },
            },
          ],
        },
      },
    },
  )
  let spread_elem = @syntax.SpreadableElem::Regular(unit_expr)
  json_inspect(
    @syntax.ArraySpread(elems=@list.from_array([spread_elem]), loc=dummy_loc).json_repr(),
    content={
      "kind": "Expr::ArraySpread",
      "loc": null,
      "children": {
        "elems": {
          "kind": "Expr::ArraySpread::ElemList",
          "loc": null,
          "children": [
            {
              "kind": "SpreadableElem::Regular",
              "loc": null,
              "children": {
                "value": {
                  "kind": "Expr::Unit",
                  "loc": null,
                  "children": { "faked": false },
                },
              },
            },
          ],
        },
      },
    },
  )
  json_inspect(
    @syntax.ArrayGet(array=unit_expr, index=unit_expr, loc=dummy_loc).json_repr(),
    content={
      "kind": "Expr::ArrayGet",
      "loc": null,
      "children": {
        "array": {
          "kind": "Expr::Unit",
          "loc": null,
          "children": { "faked": false },
        },
        "index": {
          "kind": "Expr::Unit",
          "loc": null,
          "children": { "faked": false },
        },
      },
    },
  )
  json_inspect(
    @syntax.Expr::Tuple(
      exprs=@list.from_array([unit_expr, unit_expr]),
      loc=dummy_loc,
    ).json_repr(),
    content={
      "kind": "Expr::Tuple",
      "loc": null,
      "children": {
        "exprs": {
          "kind": "Expr::Tuple::ExprList",
          "loc": null,
          "children": [
            {
              "kind": "Expr::Unit",
              "loc": null,
              "children": { "faked": false },
            },
            {
              "kind": "Expr::Unit",
              "loc": null,
              "children": { "faked": false },
            },
          ],
        },
      },
    },
  )
  json_inspect(
    @syntax.Expr::Hole(loc=dummy_loc, kind=@syntax.Todo).json_repr(),
    content={
      "kind": "Expr::Hole",
      "loc": null,
      "children": {
        "kind": { "kind": "Hole::Todo", "loc": null, "children": {} },
      },
    },
  )
  json_inspect(
    @syntax.Return(return_value=Some(unit_expr), loc=dummy_loc).json_repr(),
    content={
      "kind": "Expr::Return",
      "loc": null,
      "children": {
        "return_value": {
          "kind": "Expr::Unit",
          "loc": null,
          "children": { "faked": false },
        },
      },
    },
  )
  json_inspect(@syntax.Return(return_value=None, loc=dummy_loc).json_repr(), content={
    "kind": "Expr::Return",
    "loc": null,
    "children": { "return_value": null },
  })
  json_inspect(@syntax.Raise(err_value=unit_expr, loc=dummy_loc).json_repr(), content={
    "kind": "Expr::Raise",
    "loc": null,
    "children": {
      "err_value": {
        "kind": "Expr::Unit",
        "loc": null,
        "children": { "faked": false },
      },
    },
  })
  json_inspect(
    @syntax.Break(arg=Some(unit_expr), label=Some(label), loc=dummy_loc).json_repr(),
    content={
      "kind": "Expr::Break",
      "loc": null,
      "children": {
        "arg": {
          "kind": "Expr::Unit",
          "loc": null,
          "children": { "faked": false },
        },
        "label": {
          "kind": "Label",
          "loc": null,
          "children": { "name": "test_label" },
        },
      },
    },
  )
  json_inspect(
    @syntax.Continue(
      args=@list.from_array([unit_expr]),
      label=None,
      loc=dummy_loc,
    ).json_repr(),
    content={
      "kind": "Expr::Continue",
      "loc": null,
      "children": {
        "args": {
          "kind": "Expr::Continue::ExprList",
          "loc": null,
          "children": [
            {
              "kind": "Expr::Unit",
              "loc": null,
              "children": { "faked": false },
            },
          ],
        },
        "label": null,
      },
    },
  )
}

///|
test "expr complex types to_json" {
  let unit_expr = @syntax.Expr::Unit(loc=dummy_loc, faked=false)
  let int_type = @syntax.Name(
    constr_id=@syntax.ConstrId::{
      id: @syntax.LongIdent::Ident(name="Int"),
      loc: dummy_loc,
    },
    tys=@list.from_array([]),
    loc=dummy_loc,
  )
  let binder = @syntax.Binder::{ name: "test_binder", loc: dummy_loc }
  let label = @syntax.Label::{ name: "test_label", loc: dummy_loc }
  let any_pattern = @syntax.Pattern::Any(loc=dummy_loc)
  let case = @syntax.Case::{
    pattern: any_pattern,
    guard_: None,
    body: unit_expr,
  }
  let type_name = @syntax.TypeName::{
    name: @syntax.LongIdent::Ident(name="TestType"),
    is_object: false,
    loc: dummy_loc,
  }
  let field_def = @syntax.FieldDef::{
    label,
    expr: unit_expr,
    is_pun: false,
    loc: dummy_loc,
  }
  let variable = @syntax.Var::{
    name: @syntax.LongIdent::Ident(name="test_var"),
    loc: dummy_loc,
  }
  json_inspect(
    @syntax.If(
      cond=unit_expr,
      ifso=unit_expr,
      ifnot=Some(unit_expr),
      loc=dummy_loc,
    ).json_repr(),
    content={
      "kind": "Expr::If",
      "loc": null,
      "children": {
        "cond": {
          "kind": "Expr::Unit",
          "loc": null,
          "children": { "faked": false },
        },
        "ifso": {
          "kind": "Expr::Unit",
          "loc": null,
          "children": { "faked": false },
        },
        "ifnot": {
          "kind": "Expr::Unit",
          "loc": null,
          "children": { "faked": false },
        },
      },
    },
  )
  json_inspect(
    @syntax.Guard(
      cond=unit_expr,
      otherwise=Some(unit_expr),
      body=unit_expr,
      loc=dummy_loc,
    ).json_repr(),
    content={
      "kind": "Expr::Guard",
      "loc": null,
      "children": {
        "cond": {
          "kind": "Expr::Unit",
          "loc": null,
          "children": { "faked": false },
        },
        "otherwise": {
          "kind": "Expr::Unit",
          "loc": null,
          "children": { "faked": false },
        },
        "body": {
          "kind": "Expr::Unit",
          "loc": null,
          "children": { "faked": false },
        },
      },
    },
  )
  json_inspect(
    @syntax.Defer(expr=unit_expr, body=unit_expr, loc=dummy_loc).json_repr(),
    content={
      "kind": "Expr::Defer",
      "loc": null,
      "children": {
        "expr": {
          "kind": "Expr::Unit",
          "loc": null,
          "children": { "faked": false },
        },
        "body": {
          "kind": "Expr::Unit",
          "loc": null,
          "children": { "faked": false },
        },
      },
    },
  )
  json_inspect(
    @syntax.Is(expr=unit_expr, pat=any_pattern, loc=dummy_loc).json_repr(),
    content={
      "kind": "Expr::Is",
      "loc": null,
      "children": {
        "expr": {
          "kind": "Expr::Unit",
          "loc": null,
          "children": { "faked": false },
        },
        "pat": { "kind": "Pattern::Any", "loc": null, "children": {} },
      },
    },
  )
  json_inspect(
    @syntax.Let(
      pattern=any_pattern,
      expr=unit_expr,
      body=unit_expr,
      loc=dummy_loc,
    ).json_repr(),
    content={
      "kind": "Expr::Let",
      "loc": null,
      "children": {
        "pattern": { "kind": "Pattern::Any", "loc": null, "children": {} },
        "expr": {
          "kind": "Expr::Unit",
          "loc": null,
          "children": { "faked": false },
        },
        "body": {
          "kind": "Expr::Unit",
          "loc": null,
          "children": { "faked": false },
        },
      },
    },
  )
  json_inspect(
    @syntax.LetMut(
      binder~,
      ty=Some(int_type),
      expr=unit_expr,
      body=unit_expr,
      loc=dummy_loc,
    ).json_repr(),
    content={
      "kind": "Expr::LetMut",
      "loc": null,
      "children": {
        "binder": {
          "kind": "Binder",
          "loc": null,
          "children": { "name": "test_binder" },
        },
        "ty": {
          "kind": "Type::Name",
          "loc": null,
          "children": {
            "constr_id": {
              "kind": "ConstrId",
              "loc": null,
              "children": {
                "id": {
                  "kind": "LongIdent::Ident",
                  "loc": null,
                  "children": { "value": "Int" },
                },
              },
            },
            "tys": {
              "kind": "Type::Name::TypeList",
              "loc": null,
              "children": [],
            },
          },
        },
        "expr": {
          "kind": "Expr::Unit",
          "loc": null,
          "children": { "faked": false },
        },
        "body": {
          "kind": "Expr::Unit",
          "loc": null,
          "children": { "faked": false },
        },
      },
    },
  )
  json_inspect(
    @syntax.Expr::Sequence(
      exprs=@list.from_array([unit_expr]),
      last_expr=unit_expr,
      loc=dummy_loc,
    ).json_repr(),
    content={
      "kind": "Expr::Sequence",
      "loc": null,
      "children": {
        "exprs": {
          "kind": "Expr::Sequence::ExprList",
          "loc": null,
          "children": [
            {
              "kind": "Expr::Unit",
              "loc": null,
              "children": { "faked": false },
            },
          ],
        },
        "last_expr": {
          "kind": "Expr::Unit",
          "loc": null,
          "children": { "faked": false },
        },
      },
    },
  )
  json_inspect(
    @syntax.Expr::Record(
      type_name=Some(type_name),
      fields=@list.from_array([field_def]),
      trailing=@syntax.TrailingMark::Comma,
      loc=dummy_loc,
    ).json_repr(),
    content={
      "kind": "Expr::Record",
      "loc": null,
      "children": {
        "type_name": {
          "kind": "TypeName",
          "loc": null,
          "children": {
            "name": {
              "kind": "LongIdent::Ident",
              "loc": null,
              "children": { "value": "TestType" },
            },
            "is_object": false,
          },
        },
        "fields": {
          "kind": "Expr::Record::FieldList",
          "loc": null,
          "children": [
            {
              "kind": "FieldDef",
              "loc": null,
              "children": {
                "label": {
                  "kind": "Label",
                  "loc": null,
                  "children": { "name": "test_label" },
                },
                "expr": {
                  "kind": "Expr::Unit",
                  "loc": null,
                  "children": { "faked": false },
                },
                "is_pun": false,
              },
            },
          ],
        },
        "trailing": { "kind": "Trailing::Comma", "loc": null, "children": {} },
      },
    },
  )
  json_inspect(
    @syntax.Field(
      record=unit_expr,
      accessor=@syntax.Accessor::Label(label),
      loc=dummy_loc,
    ).json_repr(),
    content={
      "kind": "Expr::Field",
      "loc": null,
      "children": {
        "record": {
          "kind": "Expr::Unit",
          "loc": null,
          "children": { "faked": false },
        },
        "accessor": {
          "kind": "Accessor::Label",
          "loc": null,
          "children": {
            "value": {
              "kind": "Label",
              "loc": null,
              "children": { "name": "test_label" },
            },
          },
        },
      },
    },
  )
  json_inspect(
    @syntax.Expr::Match(
      expr=unit_expr,
      cases=@list.from_array([case]),
      match_loc=dummy_loc,
      loc=dummy_loc,
    ).json_repr(),
    content={
      "kind": "Expr::Match",
      "loc": null,
      "children": {
        "expr": {
          "kind": "Expr::Unit",
          "loc": null,
          "children": { "faked": false },
        },
        "cases": {
          "kind": "Expr::Match::CaseList",
          "loc": null,
          "children": [
            {
              "kind": "Case",
              "loc": null,
              "children": {
                "pattern": {
                  "kind": "Pattern::Any",
                  "loc": null,
                  "children": {},
                },
                "guard": null,
                "body": {
                  "kind": "Expr::Unit",
                  "loc": null,
                  "children": { "faked": false },
                },
              },
            },
          ],
        },
      },
    },
  )
  json_inspect(
    @syntax.Pipe(lhs=unit_expr, rhs=unit_expr, loc=dummy_loc).json_repr(),
    content={
      "kind": "Expr::Pipe",
      "loc": null,
      "children": {
        "lhs": {
          "kind": "Expr::Unit",
          "loc": null,
          "children": { "faked": false },
        },
        "rhs": {
          "kind": "Expr::Unit",
          "loc": null,
          "children": { "faked": false },
        },
      },
    },
  )
  json_inspect(
    @syntax.Assign(
      var_=variable,
      expr=unit_expr,
      augmented_by=Some(variable),
      loc=dummy_loc,
    ).json_repr(),
    content={
      "kind": "Expr::Assign",
      "loc": null,
      "children": {
        "var": {
          "kind": "Var",
          "loc": null,
          "children": {
            "name": {
              "kind": "LongIdent::Ident",
              "loc": null,
              "children": { "value": "test_var" },
            },
          },
        },
        "expr": {
          "kind": "Expr::Unit",
          "loc": null,
          "children": { "faked": false },
        },
        "augmented_by": {
          "kind": "Var",
          "loc": null,
          "children": {
            "name": {
              "kind": "LongIdent::Ident",
              "loc": null,
              "children": { "value": "test_var" },
            },
          },
        },
      },
    },
  )
}

///|
test "pattern to_json" {
  let binder = @syntax.Binder::{ name: "test_binder", loc: dummy_loc }
  let any_pattern = @syntax.Pattern::Any(loc=dummy_loc)
  let int_constant = @syntax.Int("42")
  let constant_pattern = @syntax.Pattern::Constant(
    c=int_constant,
    loc=dummy_loc,
  )
  let int_type = @syntax.Name(
    constr_id=@syntax.ConstrId::{
      id: @syntax.LongIdent::Ident(name="Int"),
      loc: dummy_loc,
    },
    tys=@list.from_array([]),
    loc=dummy_loc,
  )
  let constr_name = @syntax.ConstrName::{
    name: "TestConstructor",
    loc: dummy_loc,
  }
  let constr = @syntax.Constructor::{
    name: constr_name,
    extra_info: @syntax.NoExtraInfo,
    loc: dummy_loc,
  }
  let label = @syntax.Label::{ name: "test_field", loc: dummy_loc }
  let field_pat = @syntax.FieldPat::{
    label,
    pattern: any_pattern,
    is_pun: false,
    loc: dummy_loc,
  }
  let map_elem = @syntax.MapPatElem::{
    key: int_constant,
    pat: any_pattern,
    match_absent: false,
    key_loc: dummy_loc,
    loc: dummy_loc,
  }
  let constr_pat_arg = @syntax.ConstrPatArg::{
    pat: any_pattern,
    kind: @syntax.ArgumentKind::Positional,
  }
  let array_patterns = @syntax.ArrayPatterns::Closed(@list.from_array([]))
  json_inspect(
    @syntax.Pattern::Alias(pat=any_pattern, alias_=binder, loc=dummy_loc).json_repr(),
    content={
      "kind": "Pattern::Alias",
      "loc": null,
      "children": {
        "pat": { "kind": "Pattern::Any", "loc": null, "children": {} },
        "alias": {
          "kind": "Binder",
          "loc": null,
          "children": { "name": "test_binder" },
        },
      },
    },
  )
  json_inspect(
    @syntax.Pattern::Array(pats=array_patterns, loc=dummy_loc).json_repr(),
    content={
      "kind": "Pattern::Array",
      "loc": null,
      "children": {
        "pats": {
          "kind": "ArrayPatterns::Closed",
          "loc": null,
          "children": {
            "value": {
              "kind": "ArrayPatterns::Closed::PatternList",
              "loc": null,
              "children": [],
            },
          },
        },
      },
    },
  )
  json_inspect(
    @syntax.Expr::Constant(c=int_constant, loc=dummy_loc).json_repr(),
    content={
      "kind": "Expr::Constant",
      "loc": null,
      "children": {
        "constant": {
          "kind": "Constant::Int",
          "loc": null,
          "children": { "value": "42" },
        },
      },
    },
  )
  json_inspect(
    @syntax.Pattern::Constraint(pat=any_pattern, ty=int_type, loc=dummy_loc).json_repr(),
    content={
      "kind": "Pattern::Constraint",
      "loc": null,
      "children": {
        "pat": { "kind": "Pattern::Any", "loc": null, "children": {} },
        "ty": {
          "kind": "Type::Name",
          "loc": null,
          "children": {
            "constr_id": {
              "kind": "ConstrId",
              "loc": null,
              "children": {
                "id": {
                  "kind": "LongIdent::Ident",
                  "loc": null,
                  "children": { "value": "Int" },
                },
              },
            },
            "tys": {
              "kind": "Type::Name::TypeList",
              "loc": null,
              "children": [],
            },
          },
        },
      },
    },
  )
  json_inspect(
    @syntax.Pattern::Constr(
      constr~,
      args=Some(@list.from_array([constr_pat_arg])),
      is_open=false,
      loc=dummy_loc,
    ).json_repr(),
    content={
      "kind": "Pattern::Constr",
      "loc": null,
      "children": {
        "constr": {
          "kind": "Constructor",
          "loc": null,
          "children": {
            "name": {
              "kind": "ConstrName",
              "loc": null,
              "children": { "name": "TestConstructor" },
            },
            "extra_info": {
              "kind": "ConstructorExtraInfo::NoExtraInfo",
              "loc": null,
              "children": {},
            },
          },
        },
        "args": {
          "kind": "Pattern::Constr::ArgList",
          "loc": null,
          "children": [
            {
              "kind": "ConstrPatArg",
              "loc": null,
              "children": {
                "pat": { "kind": "Pattern::Any", "loc": null, "children": {} },
                "kind": {
                  "kind": "ArgumentKind::Positional",
                  "loc": null,
                  "children": {},
                },
              },
            },
          ],
        },
        "is_open": false,
      },
    },
  )
  json_inspect(
    @syntax.Pattern::Constr(constr~, args=None, is_open=true, loc=dummy_loc).json_repr(),
    content={
      "kind": "Pattern::Constr",
      "loc": null,
      "children": {
        "constr": {
          "kind": "Constructor",
          "loc": null,
          "children": {
            "name": {
              "kind": "ConstrName",
              "loc": null,
              "children": { "name": "TestConstructor" },
            },
            "extra_info": {
              "kind": "ConstructorExtraInfo::NoExtraInfo",
              "loc": null,
              "children": {},
            },
          },
        },
        "args": null,
        "is_open": true,
      },
    },
  )
  json_inspect(
    @syntax.Or(pat1=any_pattern, pat2=constant_pattern, loc=dummy_loc).json_repr(),
    content={
      "kind": "Pattern::Or",
      "loc": null,
      "children": {
        "pat1": { "kind": "Pattern::Any", "loc": null, "children": {} },
        "pat2": {
          "kind": "Pattern::Constant",
          "loc": null,
          "children": {
            "constant": {
              "kind": "Constant::Int",
              "loc": null,
              "children": { "value": "42" },
            },
          },
        },
      },
    },
  )
  json_inspect(
    @syntax.Pattern::Tuple(
      pats=@list.from_array([any_pattern, constant_pattern]),
      loc=dummy_loc,
    ).json_repr(),
    content={
      "kind": "Pattern::Tuple",
      "loc": null,
      "children": {
        "pats": {
          "kind": "Pattern::Tuple::PatternList",
          "loc": null,
          "children": [
            { "kind": "Pattern::Any", "loc": null, "children": {} },
            {
              "kind": "Pattern::Constant",
              "loc": null,
              "children": {
                "constant": {
                  "kind": "Constant::Int",
                  "loc": null,
                  "children": { "value": "42" },
                },
              },
            },
          ],
        },
      },
    },
  )
  json_inspect(
    @syntax.Pattern::Record(
      fields=@list.from_array([field_pat]),
      is_closed=true,
      loc=dummy_loc,
    ).json_repr(),
    content={
      "kind": "Pattern::Record",
      "loc": null,
      "children": {
        "fields": {
          "kind": "Pattern::Record::FieldList",
          "loc": null,
          "children": [
            {
              "kind": "FieldPat",
              "loc": null,
              "children": {
                "label": {
                  "kind": "Label",
                  "loc": null,
                  "children": { "name": "test_field" },
                },
                "pattern": {
                  "kind": "Pattern::Any",
                  "loc": null,
                  "children": {},
                },
                "is_pun": false,
              },
            },
          ],
        },
        "is_closed": true,
      },
    },
  )
  json_inspect(
    @syntax.Pattern::Map(
      elems=@list.from_array([map_elem]),
      is_closed=false,
      loc=dummy_loc,
    ).json_repr(),
    content={
      "kind": "Pattern::Map",
      "loc": null,
      "children": {
        "elems": {
          "kind": "Pattern::Map::ElemList",
          "loc": null,
          "children": [
            {
              "kind": "MapPatElem",
              "loc": null,
              "children": {
                "key": {
                  "kind": "Constant::Int",
                  "loc": null,
                  "children": { "value": "42" },
                },
                "pat": { "kind": "Pattern::Any", "loc": null, "children": {} },
                "match_absent": false,
              },
            },
          ],
        },
        "is_closed": false,
      },
    },
  )
  json_inspect(
    @syntax.Range(
      lhs=any_pattern,
      rhs=constant_pattern,
      kind=Inclusive,
      loc=dummy_loc,
    ).json_repr(),
    content={
      "kind": "Pattern::Range",
      "loc": null,
      "children": {
        "lhs": { "kind": "Pattern::Any", "loc": null, "children": {} },
        "rhs": {
          "kind": "Pattern::Constant",
          "loc": null,
          "children": {
            "constant": {
              "kind": "Constant::Int",
              "loc": null,
              "children": { "value": "42" },
            },
          },
        },
        "kind": { "kind": "RangeKind::Inclusive", "loc": null, "children": {} },
      },
    },
  )
}

///|
test "field_name to_json" {
  let field_name = @syntax.FieldName::{ label: "test_field", loc: dummy_loc }
  json_inspect(field_name.json_repr(), content={
    "kind": "FieldName",
    "loc": null,
    "children": { "label": "test_field" },
  })
}

///|
test "field_decl to_json" {
  let field_name = @syntax.FieldName::{ label: "test_field", loc: dummy_loc }
  let int_type = @syntax.Name(
    constr_id=@syntax.ConstrId::{
      id: @syntax.LongIdent::Ident(name="Int"),
      loc: dummy_loc,
    },
    tys=@list.from_array([]),
    loc=dummy_loc,
  )
  let field_decl = @syntax.FieldDecl::{
    name: field_name,
    ty: int_type,
    mut_: false,
    vis: @syntax.Default,
    loc: dummy_loc,
    attrs: @list.empty(),
    doc: dummy_docstring,
  }
  json_inspect(field_decl.json_repr(), content={
    "kind": "FieldDecl",
    "loc": null,
    "children": {
      "name": {
        "kind": "FieldName",
        "loc": null,
        "children": { "label": "test_field" },
      },
      "ty": {
        "kind": "Type::Name",
        "loc": null,
        "children": {
          "constr_id": {
            "kind": "ConstrId",
            "loc": null,
            "children": {
              "id": {
                "kind": "LongIdent::Ident",
                "loc": null,
                "children": { "value": "Int" },
              },
            },
          },
          "tys": { "kind": "Type::Name::TypeList", "loc": null, "children": [] },
        },
      },
      "mut": false,
      "vis": { "kind": "Visibility::Default", "loc": null, "children": {} },
      "attrs": { "kind": "FieldDecl::AttrList", "loc": null, "children": [] },
      "doc": "Test documentation",
    },
  })
}

///|
test "constr_param to_json" {
  let int_type = @syntax.Name(
    constr_id=@syntax.ConstrId::{
      id: @syntax.LongIdent::Ident(name="Int"),
      loc: dummy_loc,
    },
    tys=@list.from_array([]),
    loc=dummy_loc,
  )
  let label = @syntax.Label::{ name: "test_label", loc: dummy_loc }
  let param_with_label = @syntax.ConstrParam::{
    ty: int_type,
    mut_: false,
    label: Some(label),
  }
  let param_without_label = @syntax.ConstrParam::{
    ty: int_type,
    mut_: true,
    label: None,
  }
  json_inspect(param_with_label.json_repr(), content={
    "kind": "ConstrParam",
    "loc": null,
    "children": {
      "ty": {
        "kind": "Type::Name",
        "loc": null,
        "children": {
          "constr_id": {
            "kind": "ConstrId",
            "loc": null,
            "children": {
              "id": {
                "kind": "LongIdent::Ident",
                "loc": null,
                "children": { "value": "Int" },
              },
            },
          },
          "tys": { "kind": "Type::Name::TypeList", "loc": null, "children": [] },
        },
      },
      "mut": false,
      "label": {
        "kind": "Label",
        "loc": null,
        "children": { "name": "test_label" },
      },
    },
  })
  json_inspect(param_without_label.json_repr(), content={
    "kind": "ConstrParam",
    "loc": null,
    "children": {
      "ty": {
        "kind": "Type::Name",
        "loc": null,
        "children": {
          "constr_id": {
            "kind": "ConstrId",
            "loc": null,
            "children": {
              "id": {
                "kind": "LongIdent::Ident",
                "loc": null,
                "children": { "value": "Int" },
              },
            },
          },
          "tys": { "kind": "Type::Name::TypeList", "loc": null, "children": [] },
        },
      },
      "mut": true,
      "label": null,
    },
  })
}

///|
test "constr_decl to_json" {
  let constr_name = @syntax.ConstrName::{
    name: "TestConstructor",
    loc: dummy_loc,
  }
  let int_type = @syntax.Name(
    constr_id=@syntax.ConstrId::{
      id: @syntax.LongIdent::Ident(name="Int"),
      loc: dummy_loc,
    },
    tys=@list.from_array([]),
    loc=dummy_loc,
  )
  let label = @syntax.Label::{ name: "test_label", loc: dummy_loc }
  let param = @syntax.ConstrParam::{
    ty: int_type,
    mut_: false,
    label: Some(label),
  }
  let constr_with_args = @syntax.ConstrDecl::{
    name: constr_name,
    args: Some(@list.from_array([param])),
    tag: Some(("tag_value", dummy_loc)),
    loc: dummy_loc,
    attrs: @list.empty(),
    doc: dummy_docstring,
  }
  let constr_without_args = @syntax.ConstrDecl::{
    name: constr_name,
    args: None,
    tag: None,
    loc: dummy_loc,
    attrs: @list.empty(),
    doc: dummy_docstring,
  }
  json_inspect(constr_with_args.json_repr(), content={
    "kind": "ConstrDecl",
    "loc": null,
    "children": {
      "name": {
        "kind": "ConstrName",
        "loc": null,
        "children": { "name": "TestConstructor" },
      },
      "args": {
        "kind": "ConstrDecl::ArgList",
        "loc": null,
        "children": [
          {
            "kind": "ConstrParam",
            "loc": null,
            "children": {
              "ty": {
                "kind": "Type::Name",
                "loc": null,
                "children": {
                  "constr_id": {
                    "kind": "ConstrId",
                    "loc": null,
                    "children": {
                      "id": {
                        "kind": "LongIdent::Ident",
                        "loc": null,
                        "children": { "value": "Int" },
                      },
                    },
                  },
                  "tys": {
                    "kind": "Type::Name::TypeList",
                    "loc": null,
                    "children": [],
                  },
                },
              },
              "mut": false,
              "label": {
                "kind": "Label",
                "loc": null,
                "children": { "name": "test_label" },
              },
            },
          },
        ],
      },
      "tag": {
        "kind": "ConstrDecl::Tag",
        "loc": null,
        "children": { "value": "tag_value" },
      },
      "attrs": { "kind": "ConstrDecl::AttrList", "loc": null, "children": [] },
      "doc": "Test documentation",
    },
  })
  json_inspect(constr_without_args.json_repr(), content={
    "kind": "ConstrDecl",
    "loc": null,
    "children": {
      "name": {
        "kind": "ConstrName",
        "loc": null,
        "children": { "name": "TestConstructor" },
      },
      "args": null,
      "tag": null,
      "attrs": { "kind": "ConstrDecl::AttrList", "loc": null, "children": [] },
      "doc": "Test documentation",
    },
  })
}

///|
test "type_var_constraint to_json" {
  let constraint = @syntax.TypeVarConstraint::{
    trait_: @syntax.LongIdent::Ident(name="Show"),
    loc: dummy_loc,
  }
  json_inspect(constraint.json_repr(), content={
    "kind": "TypeVarConstraint",
    "loc": null,
    "children": {
      "trait": {
        "kind": "LongIdent::Ident",
        "loc": null,
        "children": { "value": "Show" },
      },
    },
  })
}

///|
test "type_var_binder to_json" {
  let constraint = @syntax.TypeVarConstraint::{
    trait_: @syntax.LongIdent::Ident(name="Show"),
    loc: dummy_loc,
  }
  let type_var_binder = @syntax.TypeVarBinder::{
    name: "T",
    constraints: @list.from_array([constraint]),
    name_loc: dummy_loc,
  }
  json_inspect(type_var_binder.json_repr(), content={
    "kind": "TypeVarBinder",
    "loc": null,
    "children": {
      "name": "T",
      "constraints": {
        "kind": "TypeVarBinder::TypeVarConstraintList",
        "loc": null,
        "children": [
          {
            "kind": "TypeVarConstraint",
            "loc": null,
            "children": {
              "trait": {
                "kind": "LongIdent::Ident",
                "loc": null,
                "children": { "value": "Show" },
              },
            },
          },
        ],
      },
    },
  })
}

///|
test "type_decl_binder to_json" {
  let binder_with_name = @syntax.TypeDeclBinder::{
    name: Some("TypeVar"),
    loc: dummy_loc,
  }
  let binder_without_name = @syntax.TypeDeclBinder::{
    name: None,
    loc: dummy_loc,
  }
  json_inspect(binder_with_name.json_repr(), content={
    "kind": "TypeDeclBinder",
    "loc": null,
    "children": { "name": "TypeVar" },
  })
  json_inspect(binder_without_name.json_repr(), content={
    "kind": "TypeDeclBinder",
    "loc": null,
    "children": { "name": null },
  })
}

///|
test "alias_target to_json" {
  let binder = @syntax.Binder::{ name: "test_binder", loc: dummy_loc }
  let label = @syntax.Label::{ name: "target_label", loc: dummy_loc }
  let alias_with_target = @syntax.AliasTarget::{ binder, target: Some(label) }
  let alias_without_target = @syntax.AliasTarget::{ binder, target: None }
  json_inspect(alias_with_target.json_repr(), content={
    "kind": "AliasTarget",
    "loc": null,
    "children": {
      "binder": {
        "kind": "Binder",
        "loc": null,
        "children": { "name": "test_binder" },
      },
      "target": {
        "kind": "Label",
        "loc": null,
        "children": { "name": "target_label" },
      },
    },
  })
  json_inspect(alias_without_target.json_repr(), content={
    "kind": "AliasTarget",
    "loc": null,
    "children": {
      "binder": {
        "kind": "Binder",
        "loc": null,
        "children": { "name": "test_binder" },
      },
      "target": null,
    },
  })
}

///|
test "field_pat to_json" {
  let label = @syntax.Label::{ name: "field_label", loc: dummy_loc }
  let any_pattern = @syntax.Pattern::Any(loc=dummy_loc)
  let field_pat = @syntax.FieldPat::{
    label,
    pattern: any_pattern,
    is_pun: true,
    loc: dummy_loc,
  }
  json_inspect(field_pat.json_repr(), content={
    "kind": "FieldPat",
    "loc": null,
    "children": {
      "label": {
        "kind": "Label",
        "loc": null,
        "children": { "name": "field_label" },
      },
      "pattern": { "kind": "Pattern::Any", "loc": null, "children": {} },
      "is_pun": true,
    },
  })
}

///|
test "constr_pat_arg to_json" {
  let any_pattern = @syntax.Pattern::Any(loc=dummy_loc)
  let label = @syntax.Label::{ name: "arg_label", loc: dummy_loc }
  let arg = @syntax.ConstrPatArg::{
    pat: any_pattern,
    kind: @syntax.ArgumentKind::Labelled(label),
  }
  json_inspect(arg.json_repr(), content={
    "kind": "ConstrPatArg",
    "loc": null,
    "children": {
      "pat": { "kind": "Pattern::Any", "loc": null, "children": {} },
      "kind": {
        "kind": "ArgumentKind::Labelled",
        "loc": null,
        "children": {
          "value": {
            "kind": "Label",
            "loc": null,
            "children": { "name": "arg_label" },
          },
        },
      },
    },
  })
}

///|
test "map_pat_elem to_json" {
  let int_constant = @syntax.Int("42")
  let any_pattern = @syntax.Pattern::Any(loc=dummy_loc)
  let map_elem = @syntax.MapPatElem::{
    key: int_constant,
    pat: any_pattern,
    match_absent: true,
    key_loc: dummy_loc,
    loc: dummy_loc,
  }
  json_inspect(map_elem.json_repr(), content={
    "kind": "MapPatElem",
    "loc": null,
    "children": {
      "key": {
        "kind": "Constant::Int",
        "loc": null,
        "children": { "value": "42" },
      },
      "pat": { "kind": "Pattern::Any", "loc": null, "children": {} },
      "match_absent": true,
    },
  })
}

///|
test "local_type_decl to_json" {
  let deriving_directive = @syntax.DerivingDirective::{
    type_name: @syntax.TypeName::{
      name: @syntax.LongIdent::Ident(name="Show"),
      is_object: false,
      loc: dummy_loc,
    },
    args: @list.from_array([]),
    loc: dummy_loc,
  }
  let local_type_decl = @syntax.LocalTypeDecl::{
    tycon: "TestType",
    tycon_loc: dummy_loc,
    components: @syntax.Abstract,
    deriving: @list.from_array([deriving_directive]),
  }
  json_inspect(local_type_decl.json_repr(), content={
    "kind": "LocalTypeDecl",
    "loc": null,
    "children": {
      "tycon": "TestType",
      "components": {
        "kind": "TypeDesc::Abstract",
        "loc": null,
        "children": {},
      },
      "deriving": {
        "kind": "LocalTypeDecl::DerivingList",
        "loc": null,
        "children": [
          {
            "kind": "DerivingDirective",
            "loc": null,
            "children": {
              "type_name": {
                "kind": "TypeName",
                "loc": null,
                "children": {
                  "name": {
                    "kind": "LongIdent::Ident",
                    "loc": null,
                    "children": { "value": "Show" },
                  },
                  "is_object": false,
                },
              },
              "args": {
                "kind": "DerivingDirective::ArgList",
                "loc": null,
                "children": [],
              },
            },
          },
        ],
      },
    },
  })
}

///|
test "deriving_directive to_json" {
  let unit_expr = @syntax.Expr::Unit(loc=dummy_loc, faked=false)
  let argument = @syntax.Argument::{
    value: unit_expr,
    kind: @syntax.Positional,
  }
  let deriving_directive = @syntax.DerivingDirective::{
    type_name: @syntax.TypeName::{
      name: @syntax.Dot(pkg="std", id="Show"),
      is_object: true,
      loc: dummy_loc,
    },
    args: @list.from_array([argument]),
    loc: dummy_loc,
  }
  json_inspect(deriving_directive.json_repr(), content={
    "kind": "DerivingDirective",
    "loc": null,
    "children": {
      "type_name": {
        "kind": "TypeName",
        "loc": null,
        "children": {
          "name": {
            "kind": "LongIdent::Dot",
            "loc": null,
            "children": { "pkg": "std", "id": "Show" },
          },
          "is_object": true,
        },
      },
      "args": {
        "kind": "DerivingDirective::ArgList",
        "loc": null,
        "children": [
          {
            "kind": "Argument",
            "loc": null,
            "children": {
              "value": {
                "kind": "Expr::Unit",
                "loc": null,
                "children": { "faked": false },
              },
              "kind": {
                "kind": "ArgumentKind::Positional",
                "loc": null,
                "children": {},
              },
            },
          },
        ],
      },
    },
  })
}

///|
test "type_decl to_json" {
  let type_decl_binder = @syntax.TypeDeclBinder::{
    name: Some("T"),
    loc: dummy_loc,
  }
  let type_decl = @syntax.TypeDecl::{
    tycon: "TestType",
    tycon_loc: dummy_loc,
    params: @list.from_array([type_decl_binder]),
    components: @syntax.Extern,
    attrs: @list.from_array([dummy_attribute]),
    doc: dummy_docstring,
    type_vis: @syntax.Default,
    deriving: @list.from_array([]),
    loc: dummy_loc,
    is_declare: false,
  }
  json_inspect(type_decl.json_repr(), content={
    "kind": "TypeDecl",
    "loc": null,
    "children": {
      "tycon": "TestType",
      "params": {
        "kind": "TypeDecl::ParamList",
        "loc": null,
        "children": [
          { "kind": "TypeDeclBinder", "loc": null, "children": { "name": "T" } },
        ],
      },
      "components": { "kind": "TypeDesc::Extern", "loc": null, "children": {} },
      "attrs": {
        "kind": "TypeDecl::AttrList",
        "loc": null,
        "children": [
          {
            "kind": "Attribute",
            "loc": null,
            "children": { "raw": "test_attr" },
          },
        ],
      },
      "doc": "Test documentation",
      "type_vis": { "kind": "Visibility::Default", "loc": null, "children": {} },
      "is_declare": false,
      "deriving": {
        "kind": "TypeDecl::DerivingList",
        "loc": null,
        "children": [],
      },
    },
  })
}

///|
test "decl_body to_json" {
  let unit_expr = @syntax.Expr::Unit(loc=dummy_loc, faked=false)
  let local_type_decl = @syntax.LocalTypeDecl::{
    tycon: "LocalType",
    tycon_loc: dummy_loc,
    components: @syntax.Abstract,
    deriving: @list.from_array([]),
  }
  json_inspect(
    @syntax.DeclBody(
      local_types=@list.from_array([local_type_decl]),
      expr=unit_expr,
    ).json_repr(dummy_loc),
    content={
      "kind": "DeclBody::DeclBody",
      "loc": null,
      "children": {
        "local_types": {
          "kind": "DeclBody::DeclBody::LocalTypeList",
          "loc": null,
          "children": [
            {
              "kind": "LocalTypeDecl",
              "loc": null,
              "children": {
                "tycon": "LocalType",
                "components": {
                  "kind": "TypeDesc::Abstract",
                  "loc": null,
                  "children": {},
                },
                "deriving": {
                  "kind": "LocalTypeDecl::DerivingList",
                  "loc": null,
                  "children": [],
                },
              },
            },
          ],
        },
        "expr": {
          "kind": "Expr::Unit",
          "loc": null,
          "children": { "faked": false },
        },
      },
    },
  )
  json_inspect(
    @syntax.DeclStubs(
      @syntax.Import(
        module_name=dummy_string_literal,
        func_name=dummy_string_literal,
        language=None,
      ),
    ).json_repr(dummy_loc),
    content={
      "kind": "DeclBody::DeclStubs",
      "loc": null,
      "children": {
        "value": {
          "kind": "FuncStubs::Import",
          "loc": null,
          "children": {
            "module_name": "test_string",
            "func_name": "test_string",
            "language": null,
          },
        },
      },
    },
  )
}

///|
test "fun_decl to_json" {
  let binder = @syntax.Binder::{ name: "test_func", loc: dummy_loc }
  let type_name = @syntax.TypeName::{
    name: @syntax.LongIdent::Ident(name="TestType"),
    is_object: false,
    loc: dummy_loc,
  }
  let int_type = @syntax.Name(
    constr_id=@syntax.ConstrId::{
      id: @syntax.LongIdent::Ident(name="Int"),
      loc: dummy_loc,
    },
    tys=@list.from_array([]),
    loc=dummy_loc,
  )
  let param = @syntax.Parameter::Positional(binder~, ty=Some(int_type))
  let type_var_constraint = @syntax.TypeVarConstraint::{
    trait_: @syntax.LongIdent::Ident(name="Show"),
    loc: dummy_loc,
  }
  let type_var_binder = @syntax.TypeVarBinder::{
    name: "T",
    constraints: @list.from_array([type_var_constraint]),
    name_loc: dummy_loc,
  }
  let fun_decl = @syntax.FunDecl::{
    type_name: Some(type_name),
    name: binder,
    has_error: Some(dummy_loc),
    is_async: Some(dummy_loc),
    decl_params: Some(@list.from_array([param])),
    params_loc: dummy_loc,
    quantifiers: @list.from_array([type_var_binder]),
    return_type: Some(int_type),
    error_type: @syntax.NoErrorType,
    vis: @syntax.Priv(loc=dummy_loc),
    attrs: @list.from_array([dummy_attribute]),
    doc: dummy_docstring,
  }
  json_inspect(fun_decl.json_repr(dummy_loc), content={
    "kind": "FunDecl",
    "loc": null,
    "children": {
      "type_name": {
        "kind": "TypeName",
        "loc": null,
        "children": {
          "name": {
            "kind": "LongIdent::Ident",
            "loc": null,
            "children": { "value": "TestType" },
          },
          "is_object": false,
        },
      },
      "name": {
        "kind": "Binder",
        "loc": null,
        "children": { "name": "test_func" },
      },
      "has_error": { "kind": "Raise", "loc": null, "children": {} },
      "is_async": { "kind": "Async", "loc": null, "children": {} },
      "decl_params": {
        "kind": "FunDecl::ParameterList",
        "loc": null,
        "children": [
          {
            "kind": "Parameter::Positional",
            "loc": null,
            "children": {
              "binder": {
                "kind": "Binder",
                "loc": null,
                "children": { "name": "test_func" },
              },
              "ty": {
                "kind": "Type::Name",
                "loc": null,
                "children": {
                  "constr_id": {
                    "kind": "ConstrId",
                    "loc": null,
                    "children": {
                      "id": {
                        "kind": "LongIdent::Ident",
                        "loc": null,
                        "children": { "value": "Int" },
                      },
                    },
                  },
                  "tys": {
                    "kind": "Type::Name::TypeList",
                    "loc": null,
                    "children": [],
                  },
                },
              },
            },
          },
        ],
      },
      "quantifiers": {
        "kind": "FunDecl::QuantifierList",
        "loc": null,
        "children": [
          {
            "kind": "TypeVarBinder",
            "loc": null,
            "children": {
              "name": "T",
              "constraints": {
                "kind": "TypeVarBinder::TypeVarConstraintList",
                "loc": null,
                "children": [
                  {
                    "kind": "TypeVarConstraint",
                    "loc": null,
                    "children": {
                      "trait": {
                        "kind": "LongIdent::Ident",
                        "loc": null,
                        "children": { "value": "Show" },
                      },
                    },
                  },
                ],
              },
            },
          },
        ],
      },
      "return_type": {
        "kind": "Type::Name",
        "loc": null,
        "children": {
          "constr_id": {
            "kind": "ConstrId",
            "loc": null,
            "children": {
              "id": {
                "kind": "LongIdent::Ident",
                "loc": null,
                "children": { "value": "Int" },
              },
            },
          },
          "tys": { "kind": "Type::Name::TypeList", "loc": null, "children": [] },
        },
      },
      "error_type": {
        "kind": "ErrorType::NoErrorType",
        "loc": null,
        "children": {},
      },
      "vis": { "kind": "Visibility::Priv", "loc": null, "children": {} },
      "attrs": {
        "kind": "FunDecl::AttrList",
        "loc": null,
        "children": [
          {
            "kind": "Attribute",
            "loc": null,
            "children": { "raw": "test_attr" },
          },
        ],
      },
      "doc": "Test documentation",
    },
  })
}

///|
test "trait_method_decl to_json" {
  let binder = @syntax.Binder::{ name: "trait_method", loc: dummy_loc }
  let int_type = @syntax.Name(
    constr_id=@syntax.ConstrId::{
      id: @syntax.LongIdent::Ident(name="Int"),
      loc: dummy_loc,
    },
    tys=@list.from_array([]),
    loc=dummy_loc,
  )
  let param = @syntax.Parameter::Positional(binder~, ty=Some(int_type))
  let type_var_binder = @syntax.TypeVarBinder::{
    name: "T",
    constraints: @list.from_array([]),
    name_loc: dummy_loc,
  }
  let trait_method = @syntax.TraitMethodDecl::{
    name: binder,
    has_error: None,
    is_async: None,
    quantifiers: @list.from_array([type_var_binder]),
    params: @list.from_array([param]),
    return_type: Some(int_type),
    error_type: @syntax.ErrorType::DefaultErrorType(loc=dummy_loc),
    has_default: Some(dummy_loc),
    attrs: @list.empty(),
    loc: dummy_loc,
  }
  json_inspect(trait_method.json_repr(), content={
    "kind": "TraitMethodDecl",
    "loc": null,
    "children": {
      "name": {
        "kind": "Binder",
        "loc": null,
        "children": { "name": "trait_method" },
      },
      "has_error": null,
      "is_async": null,
      "quantifiers": {
        "kind": "TraitMethodDecl::QuantifierList",
        "loc": null,
        "children": [
          {
            "kind": "TypeVarBinder",
            "loc": null,
            "children": {
              "name": "T",
              "constraints": {
                "kind": "TypeVarBinder::TypeVarConstraintList",
                "loc": null,
                "children": [],
              },
            },
          },
        ],
      },
      "params": {
        "kind": "TraitMethodDecl::ParamList",
        "loc": null,
        "children": [
          {
            "kind": "Parameter::Positional",
            "loc": null,
            "children": {
              "binder": {
                "kind": "Binder",
                "loc": null,
                "children": { "name": "trait_method" },
              },
              "ty": {
                "kind": "Type::Name",
                "loc": null,
                "children": {
                  "constr_id": {
                    "kind": "ConstrId",
                    "loc": null,
                    "children": {
                      "id": {
                        "kind": "LongIdent::Ident",
                        "loc": null,
                        "children": { "value": "Int" },
                      },
                    },
                  },
                  "tys": {
                    "kind": "Type::Name::TypeList",
                    "loc": null,
                    "children": [],
                  },
                },
              },
            },
          },
        ],
      },
      "return_type": {
        "kind": "Type::Name",
        "loc": null,
        "children": {
          "constr_id": {
            "kind": "ConstrId",
            "loc": null,
            "children": {
              "id": {
                "kind": "LongIdent::Ident",
                "loc": null,
                "children": { "value": "Int" },
              },
            },
          },
          "tys": { "kind": "Type::Name::TypeList", "loc": null, "children": [] },
        },
      },
      "error_type": {
        "kind": "ErrorType::DefaultErrorType",
        "loc": null,
        "children": {},
      },
      "has_default": {
        "kind": "TraitMethodDecl::HasDefault",
        "loc": null,
        "children": {},
      },
      "attrs": {
        "kind": "TraitMethodDecl::AttrList",
        "loc": null,
        "children": [],
      },
    },
  })
}

///|
test "trait_decl to_json" {
  let trait_binder = @syntax.Binder::{ name: "TestTrait", loc: dummy_loc }
  let super_constraint = @syntax.TypeVarConstraint::{
    trait_: @syntax.LongIdent::Ident(name="SuperTrait"),
    loc: dummy_loc,
  }
  let method_binder = @syntax.Binder::{ name: "method", loc: dummy_loc }
  let trait_method = @syntax.TraitMethodDecl::{
    name: method_binder,
    has_error: None,
    is_async: None,
    quantifiers: @list.from_array([]),
    params: @list.from_array([]),
    return_type: None,
    error_type: @syntax.NoErrorType,
    has_default: None,
    attrs: @list.empty(),
    loc: dummy_loc,
  }
  let trait_decl = @syntax.TraitDecl::{
    name: trait_binder,
    supers: @list.from_array([super_constraint]),
    methods: @list.from_array([trait_method]),
    vis: @syntax.Pub(attr=None, loc=dummy_loc),
    loc: dummy_loc,
    attrs: @list.from_array([dummy_attribute]),
    doc: dummy_docstring,
    is_declare: false,
  }
  json_inspect(trait_decl.json_repr(), content={
    "kind": "TraitDecl",
    "loc": null,
    "children": {
      "name": {
        "kind": "Binder",
        "loc": null,
        "children": { "name": "TestTrait" },
      },
      "supers": {
        "kind": "TraitDecl::SuperList",
        "loc": null,
        "children": [
          {
            "kind": "TypeVarConstraint",
            "loc": null,
            "children": {
              "trait": {
                "kind": "LongIdent::Ident",
                "loc": null,
                "children": { "value": "SuperTrait" },
              },
            },
          },
        ],
      },
      "methods": {
        "kind": "TraitDecl::MethodList",
        "loc": null,
        "children": [
          {
            "kind": "TraitMethodDecl",
            "loc": null,
            "children": {
              "name": {
                "kind": "Binder",
                "loc": null,
                "children": { "name": "method" },
              },
              "has_error": null,
              "is_async": null,
              "quantifiers": {
                "kind": "TraitMethodDecl::QuantifierList",
                "loc": null,
                "children": [],
              },
              "params": {
                "kind": "TraitMethodDecl::ParamList",
                "loc": null,
                "children": [],
              },
              "return_type": null,
              "error_type": {
                "kind": "ErrorType::NoErrorType",
                "loc": null,
                "children": {},
              },
              "has_default": null,
              "attrs": {
                "kind": "TraitMethodDecl::AttrList",
                "loc": null,
                "children": [],
              },
            },
          },
        ],
      },
      "vis": {
        "kind": "Visibility::Pub",
        "loc": null,
        "children": { "attr": null },
      },
      "is_declare": false,
      "attrs": {
        "kind": "TraitDecl::AttrList",
        "loc": null,
        "children": [
          {
            "kind": "Attribute",
            "loc": null,
            "children": { "raw": "test_attr" },
          },
        ],
      },
      "doc": "Test documentation",
    },
  })
}

///|
test "impl variants to_json" {
  let unit_expr = @syntax.Expr::Unit(loc=dummy_loc, faked=false)
  let type_decl = @syntax.TypeDecl::{
    tycon: "TestType",
    tycon_loc: dummy_loc,
    params: @list.from_array([]),
    components: @syntax.Abstract,
    attrs: @list.from_array([]),
    doc: dummy_docstring,
    type_vis: @syntax.Default,
    deriving: @list.from_array([]),
    loc: dummy_loc,
    is_declare: false,
  }
  let binder = @syntax.Binder::{ name: "test", loc: dummy_loc }
  let int_type = @syntax.Name(
    constr_id=@syntax.ConstrId::{
      id: @syntax.LongIdent::Ident(name="Int"),
      loc: dummy_loc,
    },
    tys=@list.from_array([]),
    loc=dummy_loc,
  )
  json_inspect(
    @syntax.TopExpr(
      expr=unit_expr,
      is_main=true,
      local_types=@list.from_array([]),
      is_async=None,
      loc=dummy_loc,
    ).json_repr(),
    content={
      "kind": "Impl::TopExpr",
      "loc": null,
      "children": {
        "expr": {
          "kind": "Expr::Unit",
          "loc": null,
          "children": { "faked": false },
        },
        "is_main": true,
        "local_types": {
          "kind": "Impl::TopExpr::LocalTypeList",
          "loc": null,
          "children": [],
        },
        "is_async": null,
      },
    },
  )
  json_inspect(
    @syntax.TopTest(
      expr=unit_expr,
      name=Some((dummy_string_literal, dummy_loc)),
      params=None,
      local_types=@list.from_array([]),
      is_async=None,
      loc=dummy_loc,
      attrs=@list.from_array([]),
      doc=dummy_docstring,
    ).json_repr(),
    content={
      "kind": "Impl::TopTest",
      "loc": null,
      "children": {
        "expr": {
          "kind": "Expr::Unit",
          "loc": null,
          "children": { "faked": false },
        },
        "name": {
          "kind": "TestName",
          "loc": null,
          "children": { "value": "test_string" },
        },
        "params": null,
        "local_types": {
          "kind": "Impl::TopTest::LocalTypeList",
          "loc": null,
          "children": [],
        },
        "is_async": null,
        "attrs": {
          "kind": "Impl::TopTest::AttrList",
          "loc": null,
          "children": [],
        },
        "doc": "Test documentation",
      },
    },
  )
  json_inspect(@syntax.TopTypeDef(type_decl).json_repr(), content={
    "kind": "Impl::TopTypeDef",
    "loc": null,
    "children": {
      "value": {
        "kind": "TypeDecl",
        "loc": null,
        "children": {
          "tycon": "TestType",
          "params": {
            "kind": "TypeDecl::ParamList",
            "loc": null,
            "children": [],
          },
          "components": {
            "kind": "TypeDesc::Abstract",
            "loc": null,
            "children": {},
          },
          "attrs": { "kind": "TypeDecl::AttrList", "loc": null, "children": [] },
          "doc": "Test documentation",
          "type_vis": {
            "kind": "Visibility::Default",
            "loc": null,
            "children": {},
          },
          "is_declare": false,
          "deriving": {
            "kind": "TypeDecl::DerivingList",
            "loc": null,
            "children": [],
          },
        },
      },
    },
  })
  let fun_decl = @syntax.FunDecl::{
    type_name: None,
    name: binder,
    has_error: None,
    is_async: None,
    decl_params: None,
    params_loc: dummy_loc,
    quantifiers: @list.from_array([]),
    return_type: None,
    error_type: @syntax.NoErrorType,
    vis: @syntax.Default,
    attrs: @list.from_array([]),
    doc: dummy_docstring,
  }
  let decl_body = @syntax.DeclBody(
    local_types=@list.from_array([]),
    expr=unit_expr,
  )
  json_inspect(
    @syntax.TopFuncDef(fun_decl~, decl_body~, loc=dummy_loc).json_repr(),
    content={
      "kind": "Impl::TopFuncDef",
      "loc": null,
      "children": {
        "fun_decl": {
          "kind": "FunDecl",
          "loc": null,
          "children": {
            "type_name": null,
            "name": {
              "kind": "Binder",
              "loc": null,
              "children": { "name": "test" },
            },
            "has_error": null,
            "is_async": null,
            "decl_params": null,
            "quantifiers": {
              "kind": "FunDecl::QuantifierList",
              "loc": null,
              "children": [],
            },
            "return_type": null,
            "error_type": {
              "kind": "ErrorType::NoErrorType",
              "loc": null,
              "children": {},
            },
            "vis": {
              "kind": "Visibility::Default",
              "loc": null,
              "children": {},
            },
            "attrs": {
              "kind": "FunDecl::AttrList",
              "loc": null,
              "children": [],
            },
            "doc": "Test documentation",
          },
        },
        "decl_body": {
          "kind": "DeclBody::DeclBody",
          "loc": null,
          "children": {
            "local_types": {
              "kind": "DeclBody::DeclBody::LocalTypeList",
              "loc": null,
              "children": [],
            },
            "expr": {
              "kind": "Expr::Unit",
              "loc": null,
              "children": { "faked": false },
            },
          },
        },
      },
    },
  )
  json_inspect(
    @syntax.TopLetDef(
      binder~,
      ty=Some(int_type),
      expr=unit_expr,
      vis=@syntax.Default,
      is_constant=true,
      loc=dummy_loc,
      attrs=@list.from_array([]),
      doc=dummy_docstring,
      is_declare=false,
    ).json_repr(),
    content={
      "kind": "Impl::TopLetDef",
      "loc": null,
      "children": {
        "binder": {
          "kind": "Binder",
          "loc": null,
          "children": { "name": "test" },
        },
        "ty": {
          "kind": "Type::Name",
          "loc": null,
          "children": {
            "constr_id": {
              "kind": "ConstrId",
              "loc": null,
              "children": {
                "id": {
                  "kind": "LongIdent::Ident",
                  "loc": null,
                  "children": { "value": "Int" },
                },
              },
            },
            "tys": {
              "kind": "Type::Name::TypeList",
              "loc": null,
              "children": [],
            },
          },
        },
        "expr": {
          "kind": "Expr::Unit",
          "loc": null,
          "children": { "faked": false },
        },
        "vis": { "kind": "Visibility::Default", "loc": null, "children": {} },
        "is_constant": true,
        "is_declare": false,
        "attrs": {
          "kind": "Impl::TopLetDef::AttrList",
          "loc": null,
          "children": [],
        },
        "doc": "Test documentation",
      },
    },
  )
  let view = @syntax.TopView(
    quantifiers=@list.empty(),
    source_ty=int_type,
    view_type_name="TestViewType",
    view_func_name=binder,
    view_type_loc=dummy_loc,
    params_loc=dummy_loc,
    view_constrs=@list.empty(),
    parameters=@list.empty(),
    body=unit_expr,
    vis=@syntax.Default,
    attrs=@list.empty(),
    doc=dummy_docstring,
    loc=dummy_loc,
  )
  json_inspect(view.json_repr(), content={
    "kind": "Impl::TopView",
    "loc": null,
    "children": {
      "quantifiers": {
        "kind": "Impl::TopView::QuantifierList",
        "loc": null,
        "children": [],
      },
      "source_ty": {
        "kind": "Type::Name",
        "loc": null,
        "children": {
          "constr_id": {
            "kind": "ConstrId",
            "loc": null,
            "children": {
              "id": {
                "kind": "LongIdent::Ident",
                "loc": null,
                "children": { "value": "Int" },
              },
            },
          },
          "tys": { "kind": "Type::Name::TypeList", "loc": null, "children": [] },
        },
      },
      "view_constrs": {
        "kind": "Impl::TopView::ConstrList",
        "loc": null,
        "children": [],
      },
      "body": {
        "kind": "Expr::Unit",
        "loc": null,
        "children": { "faked": false },
      },
      "ty_name": "TestViewType",
      "parameters": {
        "kind": "Impl::TopView::ParameterList",
        "loc": null,
        "children": [],
      },
      "func_name": {
        "kind": "Binder",
        "loc": null,
        "children": { "name": "test" },
      },
      "vis": { "kind": "Visibility::Default", "loc": null, "children": {} },
      "attrs": {
        "kind": "Impl::TopView::AttrList",
        "loc": null,
        "children": [],
      },
      "doc": "Test documentation",
    },
  })
}
