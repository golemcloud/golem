%{
(* Copyright International Digital Economy Academy, all rights reserved *)
[@@@coverage off]
let base = Loc.to_base_pos ~pkg:"" { pos_fname = ""; pos_lnum = 0; pos_column = 0 }
let i (start, end_) =
  Rloc.of_lex_pos ~base start end_

module Syntax = Parsing_syntax
%}

%token <Lex_literal.char_literal> CHAR
%token <string> INT
%token <Lex_literal.char_literal> BYTE
%token <Lex_literal.string_literal> BYTES
%token <string> FLOAT
%token <string> DOUBLE
%token <Lex_literal.string_literal> STRING
%token <string> MULTILINE_STRING
%token <Lex_literal.interp_literal> MULTILINE_INTERP
%token <Lex_literal.interp_literal> INTERP
%token <string> REGEX_LITERAL
%token <Lex_literal.interp_literal> REGEX_INTERP
%token <string * string option * string> ATTRIBUTE
%token <string> LIDENT
%token <string> UIDENT
%token <string> POST_LABEL
%token <Comment.t> COMMENT
%token NEWLINE
%token <string> INFIX1
%token <string> INFIX2
%token <string> INFIX3
%token <string> INFIX4
%token <string> AUGMENTED_ASSIGNMENT

%token EOF
%token FALSE
%token TRUE
%token PUB             "pub"
%token PRIV            "priv"
%token READONLY        "readonly"
%token IMPORT          "import"
%token EXTERN          "extern"
%token BREAK           "break"
%token CONTINUE        "continue"
%token STRUCT          "struct"
%token ENUM            "enum"
%token TRAIT           "trait"
%token DERIVE          "derive"
%token IMPL            "impl"
%token WITH            "with"
%token RAISE           "raise"
%token THROW           "throw"
%token TRY             "try"
%token TRY_QUESTION    "try?"
%token TRY_EXCLAMATION "try!"
%token CATCH           "catch"
// %token EXCEPT          "except"
%token ASYNC           "async"
%token TYPEALIAS       "typealias"
%token TRAITALIAS      "traitalias"
%token FNALIAS         "fnalias"
%token EQUAL           "="

%token LPAREN          "("
%token RPAREN          ")"

%token COMMA          ","
%token MINUS           "-"
%token QUESTION        "?"
%token EXCLAMATION     "!"

%token <string>DOT_LIDENT
%token <string>DOT_UIDENT
%token <int>DOT_INT
%token DOT_LPAREN      ".(" // )
%token COLONCOLON      "::"
%token COLON           ":"
%token <bool>SEMI      ";"
%token LBRACKET        "["
%token PLUS           "+"
%token RBRACKET       "]"

%token UNDERSCORE      "_"
%token BAR             "|"

%token LBRACE          "{"
%token RBRACE          "}"

%token AMPERAMPER     "&&"
%token AMPER     "&"
%token CARET     "^"
%token BARBAR          "||"
%token <string>PACKAGE_NAME
/* Keywords */

%token AS              "as"
%token PIPE            "|>"
%token ELSE            "else"
%token FN            "fn"
%token IF             "if"
%token LET            "let"
%token CONST          "const"
%token MATCH          "match"
%token USING          "using"
%token MUTABLE        "mut"
%token TYPE            "type"
%token FAT_ARROW       "=>"
%token THIN_ARROW      "->"
%token WHILE           "while"
%token RETURN          "return"
%token DOTDOT          ".."
%token RANGE_INCLUSIVE "..="
%token RANGE_EXCLUSIVE "..<"
%token ELLIPSIS        "..."
%token TEST            "test"
%token LOOP            "loop"
%token GUARD           "guard"
%token DEFER           "defer"

%token FOR             "for"
%token IN              "in"
%token IS              "is"
%token SUBERROR        "suberror"
%token AND             "and"
%token LETREC          "letrec"
%token ENUMVIEW        "enumview"
%token NORAISE         "noraise"
%token LEXMATCH        "lexmatch"
%token LEXMATCH_QUESTION "lexmatch?"

%start <Mbti.t> t

%%

t:
  | pkg=LIDENT package_name=STRING ioption(";")
    imports=imports
    sigs=sigs EOF { 
    if pkg <> "package" then assert false;
    let p0 : Lexing.position = $startpos in
    ({ package_name = Lex_literal.to_string_repr package_name
     ; imports
     ; sigs
     ; loc_ = i $sloc
     ; base_pos = Loc.to_base_pos ~pkg:!Basic_config.current_package { p0 with pos_lnum = 0; pos_column = -1 }
     }: Mbti.t)
  }

imports:
  | /* empty */ { [] }
  | "import" "(" imports=separated_nonempty_list(";", package_import) ")" ";" { imports }

package_import:
  | name=STRING { { name = Lex_literal.to_string_repr name; alias = None } }
  | name=STRING "as" alias=LIDENT 
    { ({ name = Lex_literal.to_string_repr name; alias = Some alias }: Mbti.package_import) }
sigs:
  | /* empty */ { [] }
  | s=sig_ { [ { sig_ = s; loc_ = i $sloc } ] }
  | s=sig_ ";" sigs=sigs { { sig_ = s; loc_ = i ($loc(s)) } :: sigs }

// ------------- toplevel items -------------------

sig_:
  | func_sig { Func $1 }
  | type_sig { Type $1 }
  | alias_sig { Alias $1 }
  | trait_sig { Trait $1 }
  | impl_sig { Impl $1 }
  | const_sig { Const $1 }
  | value_sig { Mbti.Value $1 }

const_sig:
  | attrs=attributes vis "const" name=uident ":" type_=type_ "=" value=constant {
    ({ name; type_; value; attrs }: Mbti.const_sig)
  }

value_sig:
  | attrs=attributes vis "let" name=lident ":" type_=type_ { ({ attrs; name; type_ }: Mbti.value_sig) }

method_self_type_coloncolon:
  | name=UIDENT "::" {
    ({ name; is_object = false; loc_ = i $loc(name) } : Mbti.method_self_type)
  }
  | "&" name=UIDENT "::" {
    ({ name; is_object = true; loc_ = i $loc(name) } : Mbti.method_self_type)
  }

func_sig:
  | attrs=attributes
    vis is_async=is_async FN type_params=loption(type_params_with_constraints)
    type_name=option(method_self_type_coloncolon) name=lident
    params=delimited("(", separated_list(",", parameter), ")") 
    "->" return_=return_type {
    ({ name; params; type_name; return_; type_params; is_async; attrs }: Mbti.func_sig)
  }

trait_method_sig:
  attrs=attributes
  name=lident
  params=delimited("(", separated_list(",", parameter), ")")
  "->" return_=return_type has_default=option(eq_underscore) {
    let has_default_ = [%p? Some _] has_default in
    ({ name; params; has_default_; return_; attrs }: Mbti.trait_method_sig)
  }

%inline eq_underscore:
  | "=" "_" {}

type_sig:
  | attrs=attributes
    vis=vis "type" t=type_decl_name_with_params {
      let name, type_params = t in
      { name; type_params; components = Ptd_abstract; vis; attrs }
    }
  | attrs=attributes
    vis=vis "suberror" type_name=UIDENT ty=option(type_) {
      let exception_decl: Parsing_syntax.exception_decl =
        match ty with | None -> No_payload | Some ty -> Single_payload ty
      in
      { name = { name = type_name; loc_ = i $loc(type_name) }; type_params = []; components = Ptd_error exception_decl; vis; attrs }
    }
  | attrs=attributes
    vis=vis "suberror" type_name=UIDENT "{" cs=separated_list(";", enum_constructor) "}" {
      let exception_decl: Parsing_syntax.exception_decl = Enum_payload cs in
      { name = { name = type_name; loc_ = i $loc(type_name) }; type_params = []; components = Ptd_error exception_decl; vis; attrs }
    }
  | attrs=attributes
    vis=vis "struct" t=type_decl_name_with_params "{" fs=separated_list(";", record_decl_field) "}" {
      let name, type_params = t in
      { name; type_params; components = Ptd_record fs; vis; attrs }
    }
  | attrs=attributes
    vis=vis "struct" t=type_decl_name_with_params "(" ts=separated_list(",", type_) ")" {
      let name, type_params = t in
      { name; type_params; components = Ptd_tuple_struct ts; vis; attrs }
    }
  | attrs=attributes
    vis=vis "enum" t=type_decl_name_with_params "{" cs=separated_list(";", enum_constructor) "}" {
      let name, type_params = t in
      ({ name; type_params; components = Ptd_variant cs; vis; attrs }: Mbti.type_sig)
    }

impl_sig:
   | attrs=attributes vis
     "impl" type_params=type_params_with_constraints trait_name=qualified_uident "for" type_=type_
     { { type_params; type_; trait_name; attrs } }
   | attrs=attributes vis
     "impl" trait_name=qualified_uident "for" type_=type_
     { { type_params = []; type_; trait_name; attrs } }

trait_sig:
  | attrs=attributes vis=vis "trait" name=uident 
    super_traits=loption(preceded(":", separated_nonempty_list("+", qualified_uident)))
    "{" methods=separated_nonempty_list(";", trait_method_sig) "}" {
    ({ attrs; name; methods; super_traits; vis } : Mbti.trait_sig)
  }
  | attrs=attributes vis=vis "trait" name=uident { ({ attrs; name; methods = []; super_traits = []; vis } : Mbti.trait_sig) }

using_binder:
  | /* empty */      { None }
  | "as" name=uident { Some name }

alias_sig:
  | attrs=attributes vis=vis "type"  t=type_decl_name_with_params "=" type_=type_ {
    let name, type_params = t in
    Mbti.Type_alias { name; type_params; type_; vis; attrs }
  }
  | attrs=attributes vis "fnalias" type_name=uident "::" name=lident {
      Mbti.Func_alias { type_name; name; attrs }
    }
  | attrs=attributes vis "using" pkg=PACKAGE_NAME "{"
      "type" target=uident name=using_binder
    "}" {
      let pkg : Mbti.name = { name = pkg; loc_ = i $loc(pkg) } in
      Mbti.Using { pkg; target; name; kind = Using_type; attrs }
    }
  | attrs=attributes vis "using" pkg=PACKAGE_NAME "{"
      "trait" target=uident name=using_binder
    "}" {
      let pkg : Mbti.name = { name = pkg; loc_ = i $loc(pkg) } in
      Mbti.Using { pkg; target; name; kind = Using_trait; attrs }
    }

// --------------------------------------------

enum_constructor:
  | attrs=attributes
    id=UIDENT
    constr_args=option(delimited("(", separated_nonempty_list(",", constructor_param), ")"))
    constr_tag=option(eq_tag) {
    let constr_name : Parsing_syntax.constr_name = { name = id; loc_ = i $loc(id) } in
    {Parsing_syntax.constr_name; constr_args; constr_tag; constr_attrs=attrs; constr_loc_ = i $sloc; constr_doc = Docstring.empty }
  }

%inline eq_tag:
  | "=" tag=INT { tag, i $loc(tag) }

constructor_param:
  | mut=option("mut") ty=type_ {
    { cparam_typ = ty; cparam_mut = [%p? Some _] mut; cparam_label = None }
  }
  (* mut label~ : Type *)
  | mut=option("mut") label_name=POST_LABEL ":" typ=type_ {
    let label : Parsing_syntax.label = { label_name; loc_ = Rloc.trim_last_char (i $loc(label_name)) } in
    { Parsing_syntax.cparam_typ = typ; cparam_mut = [%p? Some _] mut; cparam_label = Some label }
  }

record_decl_field:
  | attrs=attributes
    mutflag=option("mut") name=LIDENT ":" ty=type_ {
    {Parsing_syntax.field_name = {label = name; loc_ = i $loc(name)}; field_attrs=attrs; field_ty = ty; field_mut = mutflag <> None; field_vis = Vis_default; field_loc_ = i $sloc; field_doc = Docstring.empty }
  }

// --------------------------------------------

type_param_with_constraints:
  | name=uident { { name; constraints = [] } }
  | name=uident ":" constraints=separated_nonempty_list("+", type_constraint) {
    ({ name; constraints }: Mbti.type_param_with_constraints)
  }
type_params_with_constraints:
  | "[" params=separated_list(",", type_param_with_constraints) "]" { params }

type_param_no_constraints:
  | name=uident { Name name }
  | "_" { Mbti.Underscore (i $sloc) }
type_params_no_constraints:
  | "[" params=separated_list(",", type_param_no_constraints) "]" { params }
optional_type_params_no_constraints:
  | /* empty */ { [] }
  | type_params_no_constraints { $1 }

type_constraint:
  | qualified_uident { $1 }
  /* todo: Error? */

type_decl_name_with_params:
  | type_name=uident params=optional_type_params_no_constraints { (type_name, params) }

// --------------------------------------------

simple_type:
  | ty=simple_type "?" { Ptype_option { loc_ = i $sloc; question_loc = i $loc($2); ty } }
  (* The tuple requires at least two elements, so non_empty_list_commas is used *)
  | "(" t=type_ "," ts=separated_nonempty_list(",", type_) ")" { (Ptype_tuple { loc_ = i $sloc; tys = t::ts }) }
  | "(" t=type_ ")" { t }
  | id=qualified_uident_ params=optional_type_arguments {
    Ptype_name { loc_ = (i $sloc) ;  constr_id = id ; tys = params} }
  | "&" lid=qualified_uident_ { Ptype_object lid }
  | "_" { Parsing_syntax.Ptype_any {loc_ = i $sloc } }
  

type_:
  | ty=simple_type { ty }
  (* Arrow type input is not a tuple, it does not have arity restriction *)
  | is_async=is_async "(" t=type_ "," ts=ioption(separated_nonempty_list(",", type_)) ")" "->" rty=return_type {
    let (ty_res, ty_err) = rty in
    let ts = match ts with None -> [] | Some ts -> ts in
    Ptype_arrow{ loc_ = i $sloc ; ty_arg = t::ts ; ty_res; ty_err; is_async }
  }
  | is_async=is_async "(" ")" "->" rty=return_type {
      let (ty_res, ty_err) = rty in
      Ptype_arrow { loc_ = i $sloc ; ty_arg = [] ; ty_res; ty_err; is_async }
    }
  | is_async=is_async "(" t=type_ ")""->"  rty=return_type
      {
        let (ty_res, ty_err) = rty in
        Ptype_arrow { loc_=i($sloc); ty_arg=[t]; ty_res; ty_err; is_async }
      }

return_type:
  | t=type_ { (t, No_error_typ) }
  | t1=simple_type "noraise" { (t1, Noraise { loc_ = i $sloc }) }
  | t1=simple_type "raise" {
    (t1, Default_error_typ { loc_ = i $loc($2); is_old_syntax_ = false })
  }
  | t1=simple_type "raise" ty=error_type {
    (t1, Error_typ { ty; is_old_syntax_ = false })
  }
  | ret=simple_type "raise" "?" {
    let fake_error : Parsing_syntax.typ =
      Ptype_name
        { constr_id = { lid = Lident "Error"; loc_ = Rloc.no_location }
        ; tys = []
        ; loc_ = Rloc.no_location
        }
    in
    (ret, Maybe_error { ty = fake_error; is_old_syntax_ = false })
  }

error_type:
  | constr_id=qualified_uident_ {
      (Ptype_name { constr_id; tys = []; loc_ = constr_id.loc_ } : Parsing_syntax.typ)
    }

optional_type_arguments:
  | params = delimited("[", separated_nonempty_list(",", type_), "]") { params }
  | /* empty */ { [] }

parameter:
  | t=type_ {
    Syntax.Discard_positional { ty = Some t; loc_ = Rloc.no_location}
  }
  | label=POST_LABEL ":" t=type_ {
    Syntax.Labelled
      { binder = { binder_name = label; loc_= i $loc(label) }
      ; ty = Some t
      }
  }
  | label=label "?" ":" t=type_ {
    Syntax.Question_optional
      { binder = { binder_name = label.Syntax.label_name; loc_ = label.loc_ }
      ; ty = Some t
      }
  }

constant:
  | TRUE { Parsing_syntax.Const_bool true }
  | FALSE { Parsing_syntax.Const_bool false }
  | BYTE { Parsing_syntax.Const_byte $1 }
  | BYTES { Parsing_syntax.Const_bytes $1 }
  | CHAR { Parsing_syntax.Const_char $1 }
  | INT { Parsing_util.make_int $1 }
  | DOUBLE { Parsing_util.make_double $1 }
  | STRING { Parsing_syntax.Const_string $1 }

%inline vis:
  | /* empty */ { Parsing_syntax.Vis_default }
  | "priv"      { Parsing_syntax.Vis_priv { loc_ = i $sloc } }
  | "pub" attr=pub_attr { Parsing_syntax.Vis_pub { attr; loc_ = i $sloc } }
pub_attr:
  | /* empty */ { None }
  | "(" "readonly" ")" { Some "readonly" }
  | "(" attr=LIDENT ")" { Some attr }

%inline is_async:
  | "async"     { true }
  | /* empty */ { false }

qualified_uident:
  | id=UIDENT { { name=Lident(id); loc_ = i $sloc } }
  | ps=PACKAGE_NAME id=DOT_UIDENT { ({ name=Ldot({ pkg = ps; id}); loc_ = i $sloc }: Mbti.qualified_name) }

qualified_uident_:
  | id=UIDENT { { lid=Lident(id); loc_ = i $sloc } }
  | ps=PACKAGE_NAME id=DOT_UIDENT { ({ lid=Ldot({ pkg = ps; id}); loc_ = i $sloc }: Parsing_syntax.constrid_loc) }

uident:
  | UIDENT { ({ name = $1; loc_ = i $sloc }: Mbti.name) }

lident:
  | LIDENT { ({ name = $1; loc_ = i $sloc }: Mbti.name) }

label:
  | LIDENT { ({ label_name = $1; loc_ = i $sloc }: Mbti.label) }

%inline attributes: 
  | /* empty */               { [] } 
  | nonempty_list(attribute) { $1 } 

%inline attribute:
  | ATTRIBUTE { Parsing_util.make_attribute ~loc_:(i $sloc) $1}

