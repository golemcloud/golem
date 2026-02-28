%{
[@@@coverage off]
(* Copyright International Digital Economy Academy, all rights reserved *)
[%%use
Parsing_util.(
  ( i
  , label_to_expr
  , label_to_pat
  , make_field_def
  , make_field_pat
  , make_uminus
  , make_uplus
  , make_unot
  , make_Pexpr_array
  , make_Pexpr_constant
  , make_Pexpr_ident
  , make_interps
  , make_Pexpr_interp
  , make_Pexpr_record
  , make_Pexpr_tuple
  , make_Ppat_alias
  , make_Ppat_constr
  , make_Ppat_constant
  , make_Ppat_tuple
  , make_Ptype_option
  , make_attribute
  , make_Ptype_tuple ))]
%}

%token <Lex_literal.char_literal> CHAR
%token <string> INT
%token <Lex_literal.char_literal> BYTE
%token <Lex_literal.string_literal> BYTES
%token <string> DOUBLE
%token <string> FLOAT
%token <Lex_literal.string_literal> STRING
%token <string> MULTILINE_STRING
%token <Lex_literal.interp_literal> MULTILINE_INTERP
%token <Lex_literal.interp_literal> INTERP
%token <string> REGEX_LITERAL
%token <Lex_literal.interp_literal> REGEX_INTERP
%token <(string * string option * string)> ATTRIBUTE
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
%token <bool>SEMI
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
%token RANGE_INCLUSIVE     "..="
%token RANGE_LT_INCLUSIVE  "..<="
%token RANGE_EXCLUSIVE     "..<"
%token RANGE_INCLUSIVE_REV ">=.."
%token RANGE_EXCLUSIVE_REV ">.."
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
%token DECLARE         "declare"
%token NORAISE         "noraise"
%token WHERE           "where"
%token TRY_QUESTION    "try?"
%token TRY_EXCLAMATION "try!"
%token LEXMATCH        "lexmatch"
%token LEXMATCH_QUESTION "lexmatch?"

// Note: this token is only used by `.mbti` parser
%token PACKAGE         "package"

%right BARBAR
%right AMPERAMPER

%left BAR
%left CARET
%left AMPER

// x.f(...) should be [Pexpr_dot_apply], not [Pexpr_apply(Pexpr_field, ...)]
// these two precedences are used to resolve this.
%nonassoc prec_field
%nonassoc LPAREN
%left INFIX1 // > < == != <= >=
%left INFIX2 // << >>
%left PLUS MINUS
%left INFIX3 // * / % 
%left INFIX4 // not used
%nonassoc prec_lower_than_as
%nonassoc "as"
%nonassoc prec_apply_non_ident_fn
%nonassoc "!"

/* the precedence of "," and ")" are used to compare with prec_apply_non_ident_fn
   to make sure when parsing (a, b) => ...
   the comma and rparen are always shifted */
%nonassoc prec_lower_than_arrow_fn   
%nonassoc ","
%nonassoc ")"
%nonassoc ":"

/* to resolve ambiguities with the "with" keyword.
  E.g. `lexmatch a lexmatch? "" with longest {}` */
%nonassoc prec_LEXMATCH_QUESTION
%nonassoc WITH 

%start    structure
%start    expression


%type <Parsing_syntax.expr> expression
%type <Parsing_syntax.impl list> structure
%type <Parsing_compact.semi_expr_prop > statement
%%

non_empty_list_rev(X):
  | x = X  { [x] }
  | xs = non_empty_list_rev(X) x = X { x::xs }

non_empty_list(X):
  | xs = non_empty_list_rev(X) { List.rev xs }

non_empty_list_commas_rev(X):
  | x = X  { [x] }
  | xs=non_empty_list_commas_rev(X) "," x=X { x::xs}

non_empty_list_commas_no_trailing(X):
  | xs = non_empty_list_commas_rev(X) { List.rev xs }

non_empty_list_commas( X):
  | xs = non_empty_list_commas_rev(X) option(",") {List.rev xs}

non_empty_list_commas_with_tail (X):
  | xs = non_empty_list_commas_rev(X) "," {List.rev xs}

list_commas( X):
  | {[]}
  | non_empty_list_commas(X) {$1}

list_commas_no_trailing(X):
  | { [] }
  | non_empty_list_commas_no_trailing(X) { $1 }

non_empty_list_commas_with_trailing_info(X):
  | xs = non_empty_list_commas_rev(X) comma=option(",") { (List.rev xs, [%p? Some _] comma) }

list_commas_with_trailing_info(X):
  | {([], false)}
  | non_empty_list_commas_with_trailing_info(X) { $1 }

non_empty_list_semi_rev_aux(X):
  | x = X  { [x] }
  | xs=non_empty_list_semi_rev_aux(X) SEMI x=X { x::xs}

non_empty_list_semis_rev(X):
  | xs = non_empty_list_semi_rev_aux(X) option(SEMI) {xs}

none_empty_list_semis_rev_with_trailing_info(X):
  | xs = non_empty_list_semi_rev_aux(X) semi=option(SEMI) { (xs, [%p? Some _] semi) }

non_empty_list_semis(X):
  | non_empty_list_semis_rev(X) {List.rev $1 }

list_semis_rev(X):
  | {[]}
  | non_empty_list_semis_rev(X) {$1}

list_semis(X):
  | {[]}
  | non_empty_list_semis(X){$1}

%inline id(x): x {$1}
%inline annot: ":" t=type_ {t}
%inline opt_annot: ioption(annot) {$1}

parameter:
  (* _ : Type *)
  | "_" ty=opt_annot {
    Parsing_syntax.Discard_positional { ty; loc_ = i $loc($1) }
  }
  (* binder : Type *)
  | param_binder=binder param_annot=opt_annot {
    Parsing_syntax.Positional { binder = param_binder; ty = param_annot }
  }
  (* binder~ : Type *)
  | binder_name=POST_LABEL param_annot=opt_annot {
    let param_binder : Parsing_syntax.binder =
      { binder_name; loc_ = Rloc.trim_last_char (i $loc(binder_name)) }
    in
    Parsing_syntax.Labelled { binder = param_binder; ty = param_annot }
  }
  (* binder~ : Type = expr *)
  | binder_name=POST_LABEL param_annot=opt_annot "=" default=expr {
    let param_binder : Parsing_syntax.binder =
      { binder_name; loc_ = Rloc.trim_last_char (i $loc(binder_name)) }
    in
    Parsing_syntax.Optional { default; binder = param_binder; ty = param_annot }
  }
  (* binder? : Type = expr *)
  | binder_name=LIDENT "?" param_annot=opt_annot "=" default=expr {
    let param_binder : Parsing_syntax.binder =
      { binder_name; loc_ = i $loc(binder_name) }
    in
    Parsing_syntax.Optional { default; binder = param_binder; ty = param_annot }
  }
  (* binder? : Type *)
  | binder_name=LIDENT "?" param_annot=opt_annot {
    let param_binder : Parsing_syntax.binder =
      { binder_name; loc_ = i $loc(binder_name) }
    in
    Parsing_syntax.Question_optional { binder = param_binder; ty = param_annot }
  }
;

parameters : delimited("(",list_commas(parameter), ")") {$1}

type_parameters:
  | delimited("[",non_empty_list_commas(id(tvar_binder)), "]") { $1 }

%inline is_async:
  | "async"     { Some(i $loc($1)) }
  | /* empty */ { None }

optional_type_parameters:
  | params = option(type_parameters) {
    match params with
    | None -> []
    | Some params -> params
   }
optional_type_parameters_no_constraints:
  | params = option(delimited("[",non_empty_list_commas(id(type_decl_binder)), "]")) {
    match params with
    | None -> []
    | Some params -> params
   }
%inline optional_type_arguments:
  | params = ioption(delimited("[" ,non_empty_list_commas(type_), "]")) {
    match params with
    | None -> []
    | Some params -> params
  }
fun_binder:
 | type_name=type_name "::" func_name=LIDENT {
    let binder : Parsing_syntax.binder =
      { binder_name = func_name; loc_ = i ($loc(func_name)) }
    in
    (Some type_name, binder)
  }
  | binder { (None, $1) }
fun_header:
  attrs=attributes
  vis=visibility
  is_async=is_async
    header=fun_header_generic
    ps=option(parameters)
    ts=func_return_type
    {
      let (type_name, f), has_error, quants = header in
      let return_type, error_type = ts in
      { Parsing_syntax.type_name
      ; name = f
      ; has_error
      ; is_async
      ; quantifiers = quants
      ; decl_params = ps
      ; params_loc_=(i $loc(ps))
      ; return_type
      ; error_type
      ; vis
      ; doc_ = Docstring.empty
      ; attrs
      ; is_test_ = false
      }
    }

declare_fun_header:
  attrs=attributes
  "declare"
  vis=visibility
  is_async=is_async
    header=fun_header_generic
    ps=option(parameters)
    ts=func_return_type
    {
      let (type_name, f), has_error, quants = header in
      let return_type, error_type = ts in
      { Parsing_syntax.type_name
      ; name = f
      ; has_error
      ; is_async
      ; quantifiers = quants
      ; decl_params = ps
      ; params_loc_=(i $loc(ps))
      ; return_type
      ; error_type
      ; vis
      ; doc_ = Docstring.empty
      ; attrs
      ; is_test_ = false
      }
    }

fun_header_generic:
  | "fn" ty_params=type_parameters binder=fun_binder has_error=optional_bang {
    binder, has_error, ty_params
  }
  | "fn" binder=fun_binder has_error=optional_bang ty_params=optional_type_parameters {
    binder, has_error, ty_params
  }

local_type_decl:
  | "struct" tycon=UIDENT "{" fs=list_semis(record_decl_field) "}" deriving_=deriving_directive_list {
    ({ local_tycon = tycon; local_tycon_loc_ = i $loc(tycon); local_components = Ptd_record fs; deriving_ = deriving_ }: Parsing_syntax.local_type_decl) }
  | "struct" tycon=UIDENT "(" ts=non_empty_list_commas(type_) ")" deriving_=deriving_directive_list {
    ({ local_tycon = tycon; local_tycon_loc_ = i $loc(tycon); local_components = Ptd_tuple_struct ts; deriving_ = deriving_ }: Parsing_syntax.local_type_decl) }
  | "enum" tycon=UIDENT "{" cs=list_semis(enum_constructor) "}" deriving_=deriving_directive_list {
    ({ local_tycon = tycon; local_tycon_loc_ = i $loc(tycon); local_components = Ptd_variant cs; deriving_ = deriving_ }: Parsing_syntax.local_type_decl) }
  | "type" tycon=UIDENT ty=type_ deriving_=deriving_directive_list {
    ({ local_tycon = tycon; local_tycon_loc_ = i $loc(tycon); local_components = Ptd_newtype ty; deriving_ = deriving_ }: Parsing_syntax.local_type_decl) }

extern_fun_header:
  attrs=attributes
  vis=visibility
  "extern" language=STRING "fn"
    fun_binder=fun_binder
    has_error=optional_bang
    quants=optional_type_parameters
    ps=option(parameters)
    ts=func_return_type
    {
      let type_name, f = fun_binder in
      let return_type, error_type = ts in
      language,
      { Parsing_syntax.type_name
      ; name = f
      ; has_error
      ; is_async = None
      ; quantifiers = quants
      ; decl_params = ps
      ; params_loc_=(i $loc(ps))
      ; return_type
      ; error_type
      ; vis
      ; doc_ = Docstring.empty
      ; attrs
      ; is_test_ = false
      }
    }

block_expr: "{" ls=list_semis_rev(statement) "}" {Parsing_compact.compact_rev ls (i $sloc)}

local_types_and_stmts:
  | t=local_type_decl { ([t], Parsing_syntax.Pexpr_unit { loc_ = i $sloc; faked = true }) }
  | ls=list_semis_rev(statement) { ([], Parsing_compact.compact_rev ls (i $sloc)) }
  | t=local_type_decl SEMI rest=local_types_and_stmts { (t::(fst rest), snd rest) }

block_expr_with_local_types: "{" block=local_types_and_stmts "}" { block }

impl_body:
  | block=block_expr_with_local_types {
    let local_types, expr = block in
    Parsing_syntax.Decl_body { expr; local_types }
  }
  | "=" code=STRING {
    Parsing_syntax.Decl_stubs (Embedded { language = None; code = Code_string code })
  }

expression : expr EOF { $1 }

val_header :
  | attrs=attributes vis=visibility "let" binder=binder t=opt_annot { attrs, false, vis, binder, t}
  | attrs=attributes vis=visibility "const" binder_name=UIDENT t=opt_annot {
    attrs, true, vis, { Parsing_syntax.binder_name; loc_ = i $loc(binder_name) }, t
  }

structure : list_semis(structure_item) EOF {$1}
structure_item:
  | type_header=type_header deriving_=deriving_directive_list {
      let attrs, type_vis, is_declare, tycon, tycon_loc_, params = type_header in
      Ptop_typedef
        { tycon
        ; tycon_loc_
        ; params
        ; components = Ptd_abstract
        ; type_vis
        ; doc_ = Docstring.empty
        ; deriving_
        ; loc_ = i $sloc
        ; attrs
        ; deprecated_type_bang_ = false
        ; deprecated_type_alias_syntax_ = false
        ; is_declare
        }
    }
  | attrs=attributes type_vis=visibility
    "extern" "type" tycon=UIDENT params=optional_type_parameters_no_constraints
    deriving_=deriving_directive_list {
      let tycon_loc_ = i $loc(tycon) in
      Ptop_typedef
        { tycon
        ; tycon_loc_
        ; params
        ; components = Ptd_extern
        ; type_vis
        ; doc_ = Docstring.empty
        ; deriving_
        ; loc_ = i $sloc
        ; attrs
        ; deprecated_type_bang_ = false
        ; deprecated_type_alias_syntax_ = false
        ; is_declare = false
        }
    }
  | type_header=type_header ty=type_ deriving_=deriving_directive_list {
      let attrs, type_vis, is_declare, tycon, tycon_loc_, params = type_header in
      Ptop_typedef
        { tycon
        ; tycon_loc_
        ; params
        ; components = Ptd_newtype ty
        ; type_vis
        ; doc_ = Docstring.empty
        ; deriving_
        ; loc_ = i $sloc
        ; attrs
        ; deprecated_type_bang_ =false
        ; deprecated_type_alias_syntax_ = false
        ; is_declare
        }
    }
  | type_header=suberror_header ty=option(type_) deriving_=deriving_directive_list {
      let attrs, type_vis, is_declare, tycon, tycon_loc_, deprecated_type_bang_ = type_header in
      let exception_decl: Parsing_syntax.exception_decl =
        match ty with | None -> No_payload | Some ty -> Single_payload ty
      in
      Ptop_typedef
        { tycon
        ; tycon_loc_
        ; params = []
        ; components = Ptd_error exception_decl
        ; type_vis
        ; doc_ = Docstring.empty
        ; deriving_
        ; loc_ = i $sloc
        ; attrs
        ; deprecated_type_bang_
        ; deprecated_type_alias_syntax_ = false
        ; is_declare
        }
    }
  | type_header=suberror_header "{" cs=list_semis(enum_constructor) "}" deriving_=deriving_directive_list {
      let attrs, type_vis, is_declare, tycon, tycon_loc_, deprecated_type_bang_ = type_header in
      let exception_decl: Parsing_syntax.exception_decl = Enum_payload cs in
      Ptop_typedef
        { tycon
        ; tycon_loc_
        ; params = []
        ; components = Ptd_error exception_decl
        ; type_vis
        ; doc_ = Docstring.empty
        ; deriving_
        ; loc_ = i $sloc
        ; attrs
        ; deprecated_type_bang_
        ; deprecated_type_alias_syntax_ = false
        ; is_declare
        }
    }
  | struct_header=struct_header "{" fs=list_semis(record_decl_field) "}" deriving_=deriving_directive_list {
      let attrs, type_vis, tycon, tycon_loc_, params = struct_header in
      Ptop_typedef
        { tycon
        ; tycon_loc_
        ; params
        ; components = Ptd_record fs
        ; type_vis
        ; doc_ = Docstring.empty
        ; deriving_
        ; loc_ = i $sloc
        ; attrs
        ; deprecated_type_bang_ = false
        ; deprecated_type_alias_syntax_ = false
        ; is_declare = false
        }
    }
  | struct_header=struct_header "(" ts=non_empty_list_commas(type_) ")" deriving_=deriving_directive_list {
      let attrs, type_vis, tycon, tycon_loc_, params = struct_header in
      Ptop_typedef
        { tycon
        ; tycon_loc_
        ; params
        ; components = Ptd_tuple_struct ts
        ; type_vis
        ; doc_ = Docstring.empty
        ; deriving_
        ; loc_ = i $sloc
        ; attrs
        ; deprecated_type_bang_ = false
        ; deprecated_type_alias_syntax_ = false
        ; is_declare = false
        }
    }
  | enum_header=enum_header "{" cs=list_semis(enum_constructor) "}" deriving_=deriving_directive_list {
      let attrs, type_vis, tycon, tycon_loc_, params = enum_header in
      Ptop_typedef
        { tycon
        ; tycon_loc_
        ; params
        ; components = Ptd_variant cs
        ; type_vis
        ; doc_ = Docstring.empty
        ; deriving_
        ; loc_ = i $sloc
        ; attrs
        ; deprecated_type_bang_ = false
        ; deprecated_type_alias_syntax_ = false
        ; is_declare = false
        }
    }
  | val_header=val_header "=" expr = expr {
    let attrs, is_constant, vis, binder, ty = val_header in
    Ptop_letdef { binder; ty; expr; vis; is_constant; loc_ = i $sloc; doc_ = Docstring.empty; attrs }
  }
  | t=extern_fun_header "=" mname=STRING fname=STRING {
      let lang, decl = t in
      Parsing_syntax.Ptop_funcdef {
        loc_ = (i $sloc);
        fun_decl = decl ;
        decl_body = Decl_stubs (Import {module_name = mname; func_name = fname; language = Some lang});
      }
    }
  | t=fun_header "=" mname=STRING fname=STRING {
      Parsing_syntax.Ptop_funcdef {
        loc_ = (i $sloc);
        fun_decl = t ;
        decl_body = Decl_stubs (Import {module_name = mname; func_name = fname; language = None });
      }
    }
  | t=fun_header "=" s=STRING {
      Parsing_syntax.Ptop_funcdef {
        loc_ = (i $sloc);
        fun_decl = t ;
        decl_body = Decl_stubs (Embedded { language = None; code = Code_string s });
      }
    }
  | t=fun_header "=" xs=non_empty_list(MULTILINE_STRING) {
      Parsing_syntax.Ptop_funcdef {
        loc_ = (i $sloc);
        fun_decl = t ;
        decl_body = Decl_stubs (Embedded { language = None; code = Code_multiline_string xs });
      }
    }
  | t=extern_fun_header "=" s=STRING {
      let language, decl = t in
      Parsing_syntax.Ptop_funcdef {
        loc_ = (i $sloc);
        fun_decl = decl ;
        decl_body = Decl_stubs (Embedded { language = Some language; code = Code_string s });
      }
    }
  | t=extern_fun_header "=" xs=non_empty_list(MULTILINE_STRING) {
      let language, decl = t in
      Parsing_syntax.Ptop_funcdef {
        loc_ = (i $sloc);
        fun_decl = decl ;
        decl_body = Decl_stubs (Embedded { language = Some language; code = Code_multiline_string xs });
      }
    }
  | t=fun_header body=block_expr_with_local_types {
      let local_types, body = body in
      Parsing_syntax.Ptop_funcdef {
        loc_ = (i $sloc);
        fun_decl = t;
        decl_body = Decl_body { expr=body; local_types };
      }
    }
  | t=declare_fun_header {
      Parsing_syntax.Ptop_funcdef {
        loc_ = (i $sloc);
        fun_decl = t;
        decl_body = Decl_none;
      }
    }
  | attrs=attributes vis=visibility "fnalias" target=func_alias_targets {
    let pkg, type_name, is_list_, targets = target in
    Parsing_syntax.Ptop_func_alias
      { pkg
      ; type_name
      ; targets
      ; vis
      ; attrs
      ; is_list_
      ; doc_ = Docstring.empty
      ; loc_ = i $sloc
      }
  }
  | attrs=attributes vis=visibility "trait" name=UIDENT
    supers=option(preceded(COLON, separated_nonempty_list(PLUS, tvar_constraint)))
    "{" methods=list_semis(trait_method_decl) "}" {
      let trait_name : Parsing_syntax.binder = { binder_name = name; loc_ = i ($loc(name)) } in
      let supers =
        match supers with None -> [] | Some supers -> supers
      in
      Parsing_syntax.Ptop_trait {
        trait_name;
        trait_supers = supers;
        trait_methods = methods;
        trait_vis = vis;
        trait_loc_ = i $sloc;
        trait_doc_ = Docstring.empty;
        trait_attrs = attrs;
      }
    }
  | attrs=attributes vis=visibility "traitalias" name=UIDENT "=" target=type_name {
    let binder : Parsing_syntax.binder = { binder_name = name; loc_ = i $loc(name) } in
    let (pkg : Parsing_syntax.label option), (target : Parsing_syntax.label) =
      let loc_ = (target : Parsing_syntax.type_name).loc_ in
      match target.name with
      | Ldot { pkg; id } ->
        Some { label_name = pkg; loc_ }, { label_name = id; loc_ }
      | Lident id ->
        None, { label_name = id; loc_ }
    in
    Parsing_syntax.Ptop_batch_trait_alias
      { pkg
      ; targets = [ { binder; target = Some target } ]
      ; vis
      ; loc_ = i $sloc
      ; attrs
      ; is_list_ = false
      ; is_old_syntax_ = true
      ; doc_ = Docstring.empty
      }
  }
  | attrs=attributes vis=visibility "typealias" targets=batch_type_alias_targets {
      let is_list_, pkg, targets = targets in
      Parsing_syntax.Ptop_batch_type_alias
        { pkg; targets; vis; attrs; loc_ = i $sloc; is_list_; doc_ = Docstring.empty }
    }
  | type_header=type_header "=" ty=type_ deriving_=deriving_directive_list {
      let attrs, type_vis, is_declare, tycon, tycon_loc_, params = type_header in
      Ptop_typedef
        { tycon
        ; tycon_loc_
        ; params
        ; components = Ptd_alias ty
        ; type_vis
        ; doc_ = Docstring.empty
        ; deriving_
        ; loc_ = i $sloc
        ; attrs
        ; deprecated_type_bang_ = false
        ; deprecated_type_alias_syntax_ = false
        ; is_declare
        }
    }
  | attrs=attributes type_vis=visibility "typealias"
    target=type_
    "as" tycon=UIDENT params=optional_type_parameters_no_constraints {
      Ptop_typedef
        { tycon
        ; tycon_loc_ = i $loc(tycon)
        ; type_vis
        ; params
        ; components = Ptd_alias target
        ; attrs
        ; deriving_ = []
        ; doc_ = Docstring.empty
        ; loc_ = i $sloc
        ; deprecated_type_bang_ = false
        ; deprecated_type_alias_syntax_ = true
        ; is_declare = false
        }
    }
  | attrs=attributes vis=visibility "traitalias" targets=batch_type_alias_targets {
      let is_list_, pkg, targets = targets in
      Parsing_syntax.Ptop_batch_trait_alias
        { pkg; targets; vis; attrs; loc_ = i $sloc; is_list_; is_old_syntax_ = false; doc_ = Docstring.empty }
    }
  | attrs=attributes
    is_async=is_async "test" name=option(loced_string) params=option(parameters)
    body=block_expr_with_local_types {
      let local_types, body = body in
      Parsing_syntax.Ptop_test
        { expr = body
        ; name
        ; params
        ; local_types
        ; is_async
        ; loc_ = (i $sloc)
        ; doc_ = Docstring.empty
        ; attrs
        }
  }
  | attrs=attributes
    vis=visibility
    "impl"
      quantifiers=optional_type_parameters
      trait=type_name
    "for" self_ty=type_
    "with"
      method_name=binder
      has_error=optional_bang
      params=parameters ret_ty=func_return_type
      body=impl_body
  {
    let ret_ty, err_ty = ret_ty in
    Parsing_syntax.Ptop_impl
      { self_ty = Some self_ty
      ; trait
      ; method_name
      ; has_error
      ; quantifiers
      ; params
      ; params_loc_ = i $loc(params)
      ; ret_ty
      ; err_ty
      ; body
      ; vis
      ; loc_ = i $sloc
      ; doc_ = Docstring.empty
      ; attrs
      }
  }
  | attrs=attributes
    vis=visibility
    "impl"
      quantifiers=optional_type_parameters
      trait=type_name
    "with"
      method_name=binder
      has_error=optional_bang
      params=parameters ret_ty=func_return_type
      body=impl_body
  {
    let ret_ty, err_ty = ret_ty in
    Parsing_syntax.Ptop_impl
      { self_ty = None
      ; trait
      ; method_name
      ; has_error
      ; quantifiers
      ; params
      ; params_loc_ = i $loc(params)
      ; ret_ty
      ; err_ty
      ; body
      ; vis
      ; loc_ = i $sloc
      ; doc_ = Docstring.empty
      ; attrs
      }
  }
  | attrs=attributes
    is_declare=is_declare
    vis=visibility
    "impl"
       quantifiers=optional_type_parameters
       trait=type_name
    "for" self_ty=type_
  {
    Parsing_syntax.Ptop_impl_relation
      { self_ty; trait; quantifiers; vis; attrs; loc_ = i $sloc; doc_ = Docstring.empty; is_declare }
  }
  | attrs=attributes
    vis=visibility
    "enumview"
    quantifiers=optional_type_parameters
    view_ty_name=UIDENT
    "{" cs=list_semis(enum_constructor) "}"
    "for"
    source_ty=type_
    "with"
    view_func_name=binder
    parameters=parameters
    body=block_expr
  {
    Parsing_syntax.Ptop_view {
      quantifiers;
      source_ty;
      view_ty_name;
      view_ty_loc_ = i $loc(view_ty_name);
      view_func_name;
      parameters;
      params_loc_ = i $loc(parameters);
      view_constrs = cs;
      body;
      vis;
      loc_ = i $sloc;
      attrs;
      doc_ = Docstring.empty;
    }
  }
  | attrs=attributes
    vis=visibility
    "using"
    pkg_name=PACKAGE_NAME
    "{"
    names=list_commas(using_binder)
    "}"
  {
    Parsing_syntax.Ptop_using
      { pkg = { label_name = pkg_name; loc_ = i $loc(pkg_name) }
      ; names
      ; vis
      ; attrs
      ; loc_ = i $sloc
      ; doc_ = Docstring.empty
      }
  }

%inline attributes: 
  | /* empty */               { [] } 
  | non_empty_list(attribute) { $1 } 

%inline attribute:
  | ATTRIBUTE { make_attribute ~loc_:(i $sloc) $1}

%inline visibility:
  | /* empty */ { Parsing_syntax.Vis_default }
  | "priv"      { Parsing_syntax.Vis_priv { loc_ = i $sloc } }
  | "pub" attr=pub_attr { Parsing_syntax.Vis_pub { attr; loc_ = i $sloc } }
pub_attr:
  | /* empty */ { None }
  | "(" "readonly" ")" { Some "readonly" }
  | "(" attr=LIDENT ")" { Some attr }
%inline is_declare:
  | /* empty */ { false }
  | "declare" { true }

type_header: attrs=attributes is_declare=is_declare vis=visibility "type" tycon=UIDENT params=optional_type_parameters_no_constraints {
  attrs, vis, is_declare, tycon, i $loc(tycon), params
}
suberror_header: attrs=attributes is_declare=is_declare vis=visibility "type" "!" tycon=UIDENT {
  attrs, vis, is_declare, tycon, i $loc(tycon), true
}
| attrs=attributes is_declare=is_declare vis=visibility "suberror" tycon=UIDENT {
  attrs, vis, is_declare, tycon, i $loc(tycon), false
}
struct_header: attrs=attributes vis=visibility "struct" tycon=UIDENT params=optional_type_parameters_no_constraints {
  attrs, vis, tycon, i $loc(tycon), params
}
enum_header: attrs=attributes vis=visibility "enum" tycon=UIDENT params=optional_type_parameters_no_constraints {
  attrs, vis, tycon, i $loc(tycon), params
}

batch_type_alias_targets:
  | pkg=PACKAGE_NAME target=batch_type_alias_target(DOT_UIDENT) {
    let pkg : Parsing_syntax.label = { label_name = pkg; loc_ = i $loc(pkg) } in
    false, Some pkg, [ target ]
  }
  | pkg=PACKAGE_NAME
    ".(" targets=non_empty_list_commas(batch_type_alias_target(UIDENT)) ")" {
    let pkg : Parsing_syntax.label = { label_name = pkg; loc_ = i $loc(pkg) } in
    true, Some pkg, targets
  }
  | target=batch_type_alias_target(UIDENT) {
    false, None, [ target ]
  }

batch_type_alias_target(UIDENT_MAYBE_DOT):
  | binder_name=UIDENT_MAYBE_DOT
  {
    let binder : Parsing_syntax.binder = { binder_name; loc_ = i $loc(binder_name) } in
    ({ binder; target = None } : Parsing_syntax.alias_target)
  }
  | target_name=UIDENT_MAYBE_DOT "as" binder_name=UIDENT
    {
      let binder : Parsing_syntax.binder = { binder_name; loc_ = i $loc(binder_name) } in
      let target : Parsing_syntax.label = { label_name = target_name; loc_ = i $loc(target_name) } in
      ({ binder; target = Some target } : Parsing_syntax.alias_target)
    }

func_alias_targets:
  | type_name=ioption(func_alias_type_name(UIDENT))
    target=func_alias_target(LIDENT) {
    None, type_name, false, [ target ]
  }
  | pkg=PACKAGE_NAME target=func_alias_target(DOT_LIDENT) {
    let pkg : Parsing_syntax.label =
      { label_name = pkg; loc_ = i $loc(pkg) }
    in
    Some pkg, None, false, [ target ]
  }
  | pkg=PACKAGE_NAME
    type_name=func_alias_type_name(DOT_UIDENT)
    target=func_alias_target(LIDENT) {
    let pkg : Parsing_syntax.label =
      { label_name = pkg; loc_ = i $loc(pkg) }
    in
    Some pkg, Some type_name, false, [ target ]
  }
  | type_name=option(func_alias_type_name(UIDENT))
    "(" targets=non_empty_list_commas(func_alias_target(LIDENT)) ")" {
    None, type_name, true, targets
  }
  | pkg=PACKAGE_NAME
    ".(" targets=non_empty_list_commas(func_alias_target(LIDENT)) ")" {
    let pkg : Parsing_syntax.label =
      { label_name = pkg; loc_ = i $loc(pkg) }
    in
    Some pkg, None, true, targets
  }
  | pkg=PACKAGE_NAME
    type_name=func_alias_type_name(DOT_UIDENT)
    "(" targets=non_empty_list_commas(func_alias_target(LIDENT)) ")" {
    let pkg : Parsing_syntax.label =
      { label_name = pkg; loc_ = i $loc(pkg) }
    in
    Some pkg, Some type_name, true, targets
  }

func_alias_type_name(UIDENT_MAYBE_DOT):
  | name=UIDENT_MAYBE_DOT "::" {
    ({ label_name = name; loc_ = i $loc(name) } : Parsing_syntax.label)
  }

func_alias_target(LIDENT_MAYBE_DOT):
  | var_name=LIDENT_MAYBE_DOT binder=option(preceded("as", binder)) {
    match binder with
    | None ->
      let binder : Parsing_syntax.binder =
        { binder_name = var_name; loc_ = i $loc(var_name) }
      in
      ({ binder; target = None } : Parsing_syntax.alias_target)
    | Some binder ->
      let target : Parsing_syntax.label =
        { label_name = var_name; loc_ = i $loc(var_name) }
      in
      ({ binder; target = Some target } : Parsing_syntax.alias_target)
  }

using_binder:
  | name=LIDENT
  | name=UIDENT {
    let binder : Parsing_syntax.binder =
      { binder_name = name; loc_ = i $sloc }
    in
    ( ({ binder; target = None } : Parsing_syntax.alias_target)
    , Parsing_syntax.Using_value
    )
  }
  | target=LIDENT "as" name=LIDENT
  | target=UIDENT "as" name=UIDENT {
    let binder : Parsing_syntax.binder =
      { binder_name = name; loc_ = i $loc(name) }
    in
    let target : Parsing_syntax.label =
      { label_name = target; loc_ = i $loc(target) }
    in
    ( ({ binder; target = Some target } : Parsing_syntax.alias_target)
    , Parsing_syntax.Using_value
    )
  }
  | "type" name=UIDENT {
    let binder : Parsing_syntax.binder =
      { binder_name = name; loc_ = i $loc(name) }
    in
    ( ({ binder; target = None } : Parsing_syntax.alias_target)
    , Parsing_syntax.Using_type
    )
  }
  | "type" target=UIDENT "as" name=UIDENT {
    let binder : Parsing_syntax.binder =
      { binder_name = name; loc_ = i $loc(name) }
    in
    let target : Parsing_syntax.label =
      { label_name = target; loc_ = i $loc(target) }
    in
    ( ({ binder; target = Some target } : Parsing_syntax.alias_target)
    , Parsing_syntax.Using_type
    )
  }
  | "trait" name=UIDENT {
    let binder : Parsing_syntax.binder =
      { binder_name = name; loc_ = i $loc(name) }
    in
    ( ({ binder; target = None } : Parsing_syntax.alias_target)
    , Parsing_syntax.Using_trait
    )
  }
  | "trait" target=UIDENT "as" name=UIDENT {
    let binder : Parsing_syntax.binder =
      { binder_name = name; loc_ = i $loc(name) }
    in
    let target : Parsing_syntax.label =
      { label_name = target; loc_ = i $loc(target) }
    in
    ( ({ binder; target = Some target } : Parsing_syntax.alias_target)
    , Parsing_syntax.Using_trait
    )
  }

deriving_directive: 
  | type_name=type_name { 
      ({ type_name_ = type_name; loc_ = i $sloc; args = [] } : Parsing_syntax.deriving_directive)
    }
  | type_name=type_name "(" args=list_commas(argument) ")" { 
      ({ type_name_ = type_name; loc_ = i $sloc; args } : Parsing_syntax.deriving_directive)
    }

deriving_directive_list:
  | /* nothing */ { [] }
  | "derive" "(" list_commas(deriving_directive) ")" { $3 }

trait_method_decl:
  attrs=attributes
  is_async=is_async
  name=binder
  has_error=optional_bang
  quantifiers=optional_type_parameters
  "("
  params=list_commas(trait_method_param)
  ")"
  return_type=func_return_type
  has_default=option(preceded("=", wildcard))
  {
    let return_type, error_type = return_type in
    Parsing_syntax.Trait_method {
      name;
      has_error;
      is_async;
      quantifiers;
      params;
      return_type;
      error_type;
      has_default;
      attrs;
      loc_ = i $sloc;
    }
  }

wildcard:
  | "_" { i $loc($1) }

trait_method_param:
  | typ=type_ {
    Parsing_syntax.Discard_positional { ty = Some typ; loc_ = i $sloc }
  }
  | binder=binder ":" typ=type_ {
    Parsing_syntax.Positional { binder; ty = Some typ }
  }
  | binder_name=POST_LABEL ":" typ=type_ {
    let binder : Parsing_syntax.binder =
      { binder_name; loc_ = Rloc.trim_last_char (i $loc(binder_name)) }
    in
    Parsing_syntax.Labelled { binder; ty = Some typ }
  }
  | binder_name=LIDENT "?" ":" typ=type_ {
    let binder : Parsing_syntax.binder =
      { binder_name; loc_ = i $loc(binder_name) }
    in
    Parsing_syntax.Question_optional { binder; ty = Some typ }
  }

qual_ident:
  | i=LIDENT { Lident(i) }
  | ps=PACKAGE_NAME id=DOT_LIDENT { Ldot({ pkg = ps; id}) }

qual_ident_simple_expr:
  /* This precedence declaration is used to disambiguate between:

     1. f(g?(...)) (error to result)
     2. f(l?=...) (forward optional argument)

     To disambiguate the two, we need to look at the token after "?" (LPAREN or EQUAL).
     But Menhir has only one token lookahead, so if reduction is needed on `g`/`l`,
     Menhir will complain for shift/reduce conflict.

     To solve this problem, here we:
     - add a specialized rule for the case of `LIDENT QUESTION LPAREN ... RPAREN`.
       Since no reduction is needed on the first LIDENT, this rule will not conflict with forwarding optional argument
     - make sure the general rule SIMPLE_EXPR QUESTION LPAREN ... RPAREN does not conflict with the specialized rule.
       This is done via the precedence declaration here.
       We assign higher precedence to shifting QUESTION than the LIDENT -> qual_ident -> simple_expr reduction chain,
       so that Menhir knows that the specialized rule has higher precedence.
  */
  | i=LIDENT %prec prec_apply_non_ident_fn { Lident(i) }
  | ps=PACKAGE_NAME id=DOT_LIDENT { Ldot({ pkg = ps; id}) }

%inline qual_ident_ty_inline:
  | id=UIDENT { Basic_longident.Lident(id) }
  | ps=PACKAGE_NAME id=DOT_UIDENT { Basic_longident.Ldot({ pkg = ps; id}) }

qual_ident_ty:
  | x=qual_ident_ty_inline { x }

%inline semi_expr_semi_opt: none_empty_list_semis_rev_with_trailing_info(statement)  {
  let ls, trailing = $1 in
  (Parsing_compact.compact_rev ls (i $sloc), trailing)
}

optional_bang:
  | "!" { Some(i $sloc) }
  | { None }

// the func that appears on the rhs of letand
letand_func:
  | arrow_fn_expr { $1 }
  | anony_fn { $1 }

and_func:
  | "and" b=binder ty=opt_annot "=" e=letand_func { (b, ty, e) }

statement:
  | "let" pat=pattern ty_opt=opt_annot "=" expr=expr
    {
      let pat =
        match ty_opt with
        | None -> pat
        | Some ty ->
          Parsing_syntax.Ppat_constraint
                  {
                    pat;
                    ty;
                    loc_ =
                      Rloc.merge
                        (Parsing_syntax.loc_of_pattern pat)
                        (Parsing_syntax.loc_of_type_expression ty);
                  } in
      Stmt_let {pat; expr; loc=(i $sloc);  }}
  | "letrec" binder=binder ty=opt_annot "=" fn=letand_func rest=list(and_func) {
    Parsing_compact.Stmt_letand { bindings = (binder, ty, fn) :: rest; loc = i $sloc }
  }
  | "let" "mut" binder=binder ty=opt_annot "=" expr=expr
    { Stmt_letmut {binder; ty_opt=ty; expr; loc=(i $sloc)} }
  | is_async=is_async "fn" binder=binder has_error=optional_bang params=parameters ty_opt=func_return_type block = block_expr
    {
      (* FIXME: `func` should have explicit return type in the ast *)
      let locb = i $sloc in
      let return_type, error_type = ty_opt in
      let func : Parsing_syntax.func =
          { parameters = params
          ; params_loc_ = (i $loc(params))
          ; body = block
          ; return_type
          ; error_type
          ; kind_ = Lambda
          ; has_error
          ; is_async
          ; loc_ = locb
          }
      in
      Parsing_compact.Stmt_func {binder; func; loc=locb}
    }
  | guard_statement { $1 }
  | "defer" expr=pipe_expr {
      Parsing_compact.Stmt_defer { expr; loc = i $sloc }
    }
  | expr_statement { Parsing_compact.Stmt_expr { expr = $1 } }

guard_statement: 
  | "guard" cond=infix_expr 
    { Parsing_compact.Stmt_guard { cond; otherwise=None; loc=(i $sloc) } }
  | "guard" cond=infix_expr "else" else_=block_expr 
    { Parsing_compact.Stmt_guard { cond; otherwise=Some else_; loc=(i $sloc) } }

%inline assignment_expr:
  | lv = left_value "=" e=expr {
    let loc_ = i $sloc in
    match lv with
    | `Var var ->
      Parsing_syntax.Pexpr_assign { var; expr = e; augmented_by = None; loc_ }
    | `Field_access (record, accessor) ->
      Parsing_syntax.Pexpr_mutate { record; accessor; field = e; augmented_by = None; loc_ }
    | `Array_access (array, index) ->
      Pexpr_array_set {array; index; value=e; loc_}
  }

%inline augmented_assignment_expr:
  | lv = left_value op=assignop e=expr {
    let loc_ = i $sloc in
    match lv with
    | `Var var ->
      Parsing_syntax.Pexpr_assign { var; expr = e; augmented_by = Some op; loc_ }
    | `Field_access (record, accessor) ->
      Parsing_syntax.Pexpr_mutate { record; accessor; field = e; augmented_by = Some op; loc_ }
    | `Array_access (array, index) ->
      Pexpr_array_augmented_set {op; array; index; value=e; loc_}
  }

// this is for the body of arrow fn, where continue is conflicting:
// f(x => continue 1, 2)
// return is also conflicting
// loop x => return { ... }
expr_statement_no_break_continue_return:
  | "raise" expr = expr { Parsing_syntax.Pexpr_raise { err_value = expr; loc_ = i $sloc } }
  | "..." { Parsing_syntax.Pexpr_hole { loc_ = i $sloc; kind = Todo } }
  | augmented_assignment_expr
  | assignment_expr
  | expr { $1 }

expr_statement:
  | "break" label=POST_LABEL arg=option(expr) {
      let label = { Parsing_syntax.label_name = label; loc_ = i $loc(label) } in
      Parsing_syntax.Pexpr_break { arg; label = Some label; loc_ = i $sloc }
    }
  | "break" arg=option(expr) {
      Parsing_syntax.Pexpr_break { arg; label = None; loc_ = i $sloc }
    }
  | "continue" label=POST_LABEL args=list_commas_no_trailing(expr) {
      let label = { Parsing_syntax.label_name = label; loc_ = i $loc(label) } in
      Parsing_syntax.Pexpr_continue { args; label = Some label; loc_ = i $sloc }
    }
  | "continue" args=list_commas_no_trailing(expr) {
      Parsing_syntax.Pexpr_continue { args; label = None; loc_ = i $sloc }
    }
  | "return" expr = option(expr) { Parsing_syntax.Pexpr_return { return_value = expr; loc_ = i $sloc } }
  | expr_statement_no_break_continue_return { $1 }

loop_label_colon:
  | label=POST_LABEL ":" { Some { Parsing_syntax.label_name = label; loc_ = i $loc(label) } }
  | { None }

while_expr:
  | label=loop_label_colon "while" cond=infix_expr body=block_expr while_else=optional_else
    { Parsing_syntax.Pexpr_while { loc_=(i $sloc); loop_cond = cond; loop_body = body; while_else; label } }

single_pattern_case:
  | pat=pattern guard=option(preceded("if", infix_expr)) "=>" body=expr_statement
   { ({ pattern = pat; guard; body }: Parsing_syntax.case) }
  | "..."
   { ({ pattern = Ppat_any { loc_ = i $sloc }; guard = None; body = Pexpr_hole { loc_ = i $sloc; kind = Todo } }: Parsing_syntax.case) }

single_pattern_cases:
| cases=list_semis(single_pattern_case) { cases }

catch_keyword:
  | "catch" "{"  { false, i $sloc }
  | "catch" "!" "{" { true, i $sloc }

%inline else_keyword:
  | "else" "{" { true, i $sloc }
  | "noraise" "{" { false, i $sloc }

try_expr:
  | "try" body=pipe_expr catch_keyword=catch_keyword catch=single_pattern_cases "}"
    { let catch_all, catch_loc_ = catch_keyword in
      Parsing_syntax.Pexpr_try { loc_=(i $sloc); body; catch; catch_all; try_else = None;
                                 else_loc_ = Rloc.no_location; legacy_else_ = false; try_loc_ = i $loc($1); catch_loc_; has_try_ = true } }
  | "try" body=pipe_expr catch_keyword=catch_keyword catch=single_pattern_cases "}"
    else_loc_=else_keyword try_else=single_pattern_cases "}"
    { let catch_all, catch_loc_ = catch_keyword in
      let legacy_else_, else_loc_ = else_loc_ in
      Parsing_syntax.Pexpr_try { loc_=(i $sloc); body; catch; catch_all; try_else = Some try_else;
                                 else_loc_; legacy_else_; try_loc_ = i $loc($1); catch_loc_; has_try_ = true } }
  | "try?" body=pipe_expr {
      Parsing_syntax.Pexpr_try_operator { body; try_loc_ = i $loc($1); loc_ = i $sloc; try_operator_kind = Parsing_syntax.Try_question }
    }
  | "try!" body=pipe_expr {
      Parsing_syntax.Pexpr_try_operator { body; try_loc_ = i $loc($1); loc_ = i $sloc; try_operator_kind = Parsing_syntax.Try_exclamation }
    }

if_expr:
  | "if"  b=infix_expr ifso=block_expr "else" ifnot=block_expr 
  | "if"  b=infix_expr ifso=block_expr "else" ifnot=if_expr { Pexpr_if {loc_=(i $sloc);  cond=b; ifso; ifnot =  Some ifnot} } 
  | "if"  b=infix_expr ifso=block_expr {Parsing_syntax.Pexpr_if {loc_=(i $sloc); cond = b; ifso; ifnot =None}}  

%inline match_header:
  | "match" e=infix_expr "{" { e }

match_expr:
  | e=match_header mat=non_empty_list_semis( single_pattern_case )  "}"  {
    Pexpr_match {loc_=(i $sloc);  expr = e ; cases =  mat; match_loc_ = i $loc(e)} }
  | e=match_header "}" { Parsing_syntax.Pexpr_match {loc_ = (i $sloc) ; expr = e ; cases =  []; match_loc_ = i $loc(e)}}

lexmatch_expr:
  | lexmatch_header list_semis(lex_case) "}" { (Pexpr_lexmatch {strategy = snd $1; expr = fst $1; match_loc_ = i $loc($1); cases = $2; loc_ = i $loc} : Parsing_syntax.expr) }

lexmatch_header:
  | "lexmatch" infix_expr "{" { ($2, None) }
  | "lexmatch" infix_expr "with" label "{" { ($2, Some $4) }

lex_case:
  | lex_pattern "=>" expr_statement { ({pat = $1; pat_loc_ = i $loc($1); body = $3} : Parsing_syntax.lex_case) }
  | "..." { {pat = [Pltop_wildcard { loc_ = i $loc }]; pat_loc_ = i $sloc; body = Pexpr_hole { loc_ = i $sloc; kind = Todo }} }

lex_pattern:
  | "(" separated_nonempty_list(",", lex_top_pattern) ")" { $2 }
  | "_" { [Pltop_wildcard { loc_ = i $loc }] }
  | binder { [Pltop_binder $1] }
  | lex_simple_atom_pattern { [(Pltop_pattern $1 : Parsing_syntax.lex_top_pattern)] }

lex_top_pattern:
  | lex_as_pattern { Pltop_pattern $1 }
  | "_" { Pltop_wildcard { loc_ = i $loc } }
  | binder { (Pltop_binder $1 : Parsing_syntax.lex_top_pattern) }
  ;

lex_as_pattern:
  | lex_pattern_sequence { match $1 with [pat] -> pat | _ -> (Plpat_sequence {pats = $1; loc_ = i $loc} : Parsing_syntax.lex_pattern) }
  | lex_atom_pattern "as" binder { (Plpat_alias {pat = $1; binder = $3; loc_ = i $loc} : Parsing_syntax.lex_pattern) }

lex_pattern_sequence:
  | lex_atom_pattern { [$1] }
  | lex_atom_pattern option(SEMI) lex_pattern_sequence { $1 :: $3 }

lex_atom_pattern:
  | lex_simple_atom_pattern { $1 }
  | "(" lex_as_pattern ")" { $2 }

lex_simple_atom_pattern:
  | REGEX_LITERAL { (Plpat_regex {lit = $1; loc_ = i $loc} : Parsing_syntax.lex_pattern) }
  | REGEX_INTERP { (Plpat_regex_interp {elems = Parsing_util.make_interps $1; loc_ = i $loc} : Parsing_syntax.lex_pattern) }
  | STRING { (Plpat_regex {lit = Lex_literal.to_string_repr $1; loc_ = i $loc} : Parsing_syntax.lex_pattern) }
  | INTERP { (Plpat_regex_interp {elems = Parsing_util.make_interps $1; loc_ = i $loc} : Parsing_syntax.lex_pattern) }

%inline loop_header:
  | "loop" arg=infix_expr "{" { arg }

loop_expr:
  | label=loop_label_colon arg=loop_header
      body=list_semis( single_pattern_case )
    "}"
    { Parsing_syntax.Pexpr_loop { arg; body; label; loc_ = i $sloc; loop_loc_ = i $loc(arg) } }

for_binders:
  | binders=list_commas_no_trailing(separated_pair(binder, "=", expr)) { binders }

optional_else:
  | "else" else_=block_expr { Some else_ }
  | { None }

where_clause_field:
  | l=label ":" e=expr { make_field_def ~loc_:(i $sloc) l e false }

optional_where_clause:
  | "where" "{" fields=list_commas(where_clause_field) "}"
    { Some ({ Parsing_syntax.fields; loc_ = i $sloc } : Parsing_syntax.where_clause) }
  | { None }

for_expr:
  | label=loop_label_colon _for_kw="for" binders = for_binders SEMI
          condition = option(infix_expr) _continue_semi=SEMI
          continue_block = list_commas_no_trailing(separated_pair(binder, "=", expr))
          body = block_expr
          for_else = optional_else
          where_clause = optional_where_clause
    { let for_loc_end = match continue_block with [] -> $endpos(_continue_semi) | _ -> $endpos(continue_block) in
      Parsing_syntax.Pexpr_for {loc_ = i $sloc; binders; condition; continue_block; body; for_else; where_clause; label; for_loc_ = i ($startpos(_for_kw), for_loc_end) } }
  | label=loop_label_colon _for_kw="for" binders = for_binders body = block_expr for_else=optional_else where_clause=optional_where_clause
    { Parsing_syntax.Pexpr_for {loc_ = i $sloc; binders; condition = None; continue_block = []; body; for_else; where_clause; label; for_loc_ = i ($startpos(_for_kw), $endpos(binders)) } }

foreach_expr:
  | label=loop_label_colon "for" binders=non_empty_list_commas(foreach_binder) "in" expr=expr
      body=block_expr
      else_block=optional_else
   {
     Parsing_syntax.Pexpr_foreach { binders; expr; body; else_block; label; loc_ = i $sloc }
   }

foreach_binder :
  | binder { Some $1 }
  | "_" { None }

expr: 
  | loop_expr
  | for_expr
  | foreach_expr
  | while_expr
  | try_expr 
  | if_expr 
  | match_expr
  | lexmatch_expr
  | simple_try_expr {$1}
  | func=arrow_fn_expr { Pexpr_function { loc_ = i $sloc; func } }


simple_try_expr:
  | body=pipe_expr catch_keyword=catch_keyword catch=single_pattern_cases "}"
    { let catch_all, catch_loc_ = catch_keyword in
      Parsing_syntax.Pexpr_try { loc_=(i $sloc); body; catch; catch_all; try_else = None; has_try_ = false;
                                 else_loc_ = Rloc.no_location; legacy_else_ = false; try_loc_ = i $loc(body); catch_loc_ } }
  | pipe_expr {$1}
  
arrow_fn_expr:
  | "(" bs=arrow_fn_prefix "=>" body=expr_statement_no_break_continue_return {
    let params_loc_ = Rloc.merge (i $loc($1)) (i $loc(bs)) in
    Parsing_util.make_arrow_fn ~params_loc_ ~loc_:(i $sloc) bs body 
  }
  | "(" ")" "=>" body=expr_statement_no_break_continue_return {
    let params_loc_ = Rloc.merge (i $loc($1)) (i $loc($2)) in
    Parsing_util.make_arrow_fn ~params_loc_ ~loc_:(i $sloc) [] body 
  }
  | b=binder "=>" body=expr_statement_no_break_continue_return { Parsing_util.make_arrow_fn ~params_loc_:(i $loc(b)) ~loc_:(i $sloc) [Parsing_util.Named b, None] body }
  | _l="_" "=>" body=expr_statement_no_break_continue_return { Parsing_util.make_arrow_fn ~params_loc_:(i $loc(_l)) ~loc_:(i $sloc) [Parsing_util.Unnamed(i $loc(_l)), None] body }


arrow_fn_prefix:
  | b=binder ioption(",") ")" { [ Parsing_util.Named b, None ] }
  | _l="_" ioption(",") ")" { [ Parsing_util.Unnamed(i $loc(_l)), None ] }
  | b=binder ":" t=type_ ioption(",") ")" { [ Parsing_util.Named b, Some t ] }
  | _l="_" ":" t=type_ ioption(",") ")" { [ Parsing_util.Unnamed(i $loc(_l)), Some t ] }
  | b=binder "," bs=arrow_fn_prefix { (Parsing_util.Named b, None)::bs }
  | _l="_" "," bs=arrow_fn_prefix { (Parsing_util.Unnamed(i $loc(_l)), None)::bs }
  | b=binder ":" t=type_ "," bs=arrow_fn_prefix { (Parsing_util.Named b, Some t):: bs }
  | _l="_" ":" t=type_ "," bs=arrow_fn_prefix { (Parsing_util.Unnamed(i $loc(_l)), Some t):: bs }

arrow_fn_prefix_no_constraint:
  | b=binder ioption(",") ")" { [ Parsing_util.Named b ] }
  | _l="_" ioption(",") ")" { [ Parsing_util.Unnamed(i $loc(_l)) ] }
  | b=binder "," bs=arrow_fn_prefix_no_constraint { Parsing_util.Named b::bs }
  | _l="_" "," bs=arrow_fn_prefix_no_constraint { Parsing_util.Unnamed(i $loc(_l))::bs }

pipe_expr: 
  | lhs=pipe_expr "|>" rhs=infix_expr {
    Parsing_syntax.Pexpr_pipe { lhs; rhs; loc_ = i $sloc }
  }
  | lhs=pipe_expr "|>" binder=binder "=>" body=block_expr {
    let params_loc_ = i $loc(binder) in
    let fn_loc = i ($startpos(binder), $endpos(body)) in
    let func = Parsing_util.make_arrow_fn ~params_loc_ ~loc_:fn_loc [Parsing_util.Named binder, None] body in
    let rhs = Parsing_syntax.Pexpr_function { loc_ = fn_loc; func } in
    Parsing_syntax.Pexpr_pipe { lhs; rhs; loc_ = i $sloc }
  }
  | infix_expr { $1 }


infix_expr:
  | lhs=infix_expr op=infixop rhs=infix_expr {
     Pexpr_infix{ op  ; lhs ; rhs ; loc_ = i($sloc)}
  }
  | postfix_expr { $1 } 

postfix_expr:
  | expr=range_expr "as" trait=type_name {
      Pexpr_as { expr; trait; loc_ = i $sloc }
    }
  | expr=range_expr "is" pat=range_pattern {
      Pexpr_is { expr; pat; loc_ = i $sloc }
    }
  | expr=range_expr "lexmatch?" pat=lex_pattern %prec prec_LEXMATCH_QUESTION {
    Pexpr_is_lexmatch { expr; pat = pat; pat_loc_ = i $loc(pat); strategy = None; loc_ = i $sloc }
  }
  | expr=range_expr "lexmatch?" pat=lex_pattern "with" label=label {
    Pexpr_is_lexmatch { expr; pat = pat; pat_loc_ = i $loc(pat); strategy = Some(label); loc_ = i $sloc }
  }
  | range_expr { $1 }

range_expr:
  | lhs=prefix_expr _op="..<" rhs=prefix_expr
   { Parsing_syntax.Pexpr_infix { op = {var_name = Lident "..<"; loc_ =  i $loc(_op)}; lhs; rhs; loc_ = i $sloc } }
  | lhs=prefix_expr _op="..=" rhs=prefix_expr
   { Parsing_syntax.Pexpr_infix { op = {var_name = Lident "..="; loc_ =  i $loc(_op)}; lhs; rhs; loc_ = i $sloc } }
  | lhs=prefix_expr _op="..<=" rhs=prefix_expr
   { Parsing_syntax.Pexpr_infix { op = {var_name = Lident "..<="; loc_ =  i $loc(_op)}; lhs; rhs; loc_ = i $sloc } }
  | lhs=prefix_expr _op=">.." rhs=prefix_expr
   { Parsing_syntax.Pexpr_infix { op = {var_name = Lident ">.."; loc_ =  i $loc(_op)}; lhs; rhs; loc_ = i $sloc } }
  | lhs=prefix_expr _op=">=.." rhs=prefix_expr
   { Parsing_syntax.Pexpr_infix { op = {var_name = Lident ">=.."; loc_ =  i $loc(_op)}; lhs; rhs; loc_ = i $sloc } }
  | prefix_expr { $1 }

prefix_expr:
  | op=id(plus) e=prefix_expr { make_uplus ~loc_:(i $sloc) op e }
  | op=id(minus) e=prefix_expr { make_uminus ~loc_:(i $sloc) op e }
  | "!" e=prefix_expr { make_unot ~loc_:(i $sloc) e}
  | simple_expr { $1 }

%inline plus:
  | PLUS { "+" }

%inline minus:
  | MINUS { "-" }

left_value:
 | var=var { `Var var }
 | record=simple_expr  acc=accessor {
     `Field_access (record, acc)
 }
 | obj=simple_expr  "[" ind=expr "]" {
    `Array_access (obj, ind)
 }

constr:
  | name = UIDENT {
     { Parsing_syntax.constr_name = { name; loc_ = i $loc(name) }
     ; extra_info = No_extra_info
     ; loc_=(i $loc)
     }
    }
  | pkg=PACKAGE_NAME constr_name=DOT_UIDENT {
      { Parsing_syntax.constr_name = { name = constr_name; loc_ = i $loc(constr_name) }
      ; extra_info = Package pkg
      ; loc_= i $sloc
      }
    }
  /* TODO: two tokens or one token here? */
  | type_name=type_name "::" constr_name=UIDENT {
      { Parsing_syntax.constr_name = { name = constr_name; loc_ = i $loc(constr_name) }
      ; extra_info = Type_name type_name
      ; loc_= i $sloc
      }
    }


%inline apply_attr:
  | { Parsing_syntax.No_attr }
  | "!" { Parsing_syntax.Exclamation }

non_empty_tuple_elems:
  | e=expr ioption(",") ")" { [e] }
  | e=expr "," es=non_empty_tuple_elems { e::es }

non_empty_tuple_elems_with_prefix:
  | b=binder "," es=non_empty_tuple_elems_with_prefix { Parsing_util.binder_to_expr b::es }
  | _l="_" "," es=non_empty_tuple_elems_with_prefix { Pexpr_hole { loc_ = i $loc(_l); kind = Incomplete }::es }
  | es=non_empty_tuple_elems { es }

tuple_expr:
  | "(" ps=arrow_fn_prefix_no_constraint {
    let es = Basic_lst.map ps Parsing_util.arrow_fn_param_to_expr in
    match es with
      | [ Pexpr_constraint _ as e ] -> e
      | [expr] -> Pexpr_group { expr; group = Group_paren; loc_ = i $sloc }
      | _ -> make_Pexpr_tuple ~loc_:(i $sloc) es
  }
  | "(" es=non_empty_tuple_elems_with_prefix { 
    match es with
    | [expr] -> Pexpr_group { expr; group = Group_paren; loc_ = i $sloc }
    | _ -> make_Pexpr_tuple ~loc_:(i $sloc) es }
  | "(" b=binder ":" t=type_ ")" {
    Parsing_syntax.Pexpr_constraint {loc_=(i $sloc); expr=Parsing_util.binder_to_expr b; ty=t}
  }
  | "(" _l="_" ":" t=type_ ")" {
    Parsing_syntax.Pexpr_constraint {loc_=(i $sloc); expr=Pexpr_hole {loc_ = i $loc(_l); kind = Incomplete}; ty=t}
  }
  | "(" e=expr ":" t=type_ ")" {
    Parsing_syntax.Pexpr_constraint {loc_=(i $sloc); expr=e; ty=t}
  }
  | "(" ")" { Parsing_syntax.Pexpr_unit {loc_ = i $sloc; faked = false} }

anony_fn:
  | is_async=is_async "fn" has_error=optional_bang ps=parameters ty_opt=func_return_type f=block_expr
    { let return_type, error_type = ty_opt in
       { parameters = ps
              ; has_error
              ; is_async
              ; params_loc_ = (i $loc(ps))
              ; body = f
              ; return_type
              ; error_type
              ; kind_ = Lambda
              ; loc_ = i $sloc
              }}

simple_expr:
  | "{" x=record_defn "}" {
      let (fs, trailing) = x in
      make_Pexpr_record ~loc_:(i $sloc) ~trailing None (fs)
    }
  | type_name=type_name COLONCOLON "{" x=list_commas_with_trailing_info(record_defn_single) "}" {
      let (fs, trailing) = x in
      let trailing = if trailing then Parsing_syntax.Trailing_comma else Parsing_syntax.Trailing_none in
      make_Pexpr_record ~loc_:(i $sloc) ~trailing (Some type_name) fs
    }
  | type_name=ioption(terminated(type_name, COLONCOLON)) "{" ".." oe=expr "}" {
      Pexpr_record_update { type_name; record=oe; fields=[]; loc_=i $sloc }
    }
  | type_name=ioption(terminated(type_name, COLONCOLON)) "{" ".." oe=expr "," fs=list_commas(record_defn_single) "}" {
      Pexpr_record_update { type_name; record=oe; fields=fs; loc_=i $sloc }
    }
  | "{" x=semi_expr_semi_opt "}" {
      match x with
      | Parsing_syntax.Pexpr_ident { id = { var_name = Lident str; loc_ }; _ } as expr, trailing ->
         let label = { Parsing_syntax.label_name = str; loc_ } in
         let field = make_field_def ~loc_:(i $sloc) label expr true in
         let trailing = if trailing then Parsing_syntax.Trailing_semi else Parsing_syntax.Trailing_none in
         make_Pexpr_record ~loc_:(i $sloc) ~trailing None [field]
      | expr, _ -> Pexpr_group { expr; group = Group_brace; loc_ = i $sloc }
    }
  | "{" elems=list_commas(map_expr_elem) "}" {
      Parsing_syntax.Pexpr_map { elems; loc_ = i $sloc }
    }
  | a = anony_fn { Pexpr_function { loc_ = i $sloc; func = a } }
  | e = atomic_expr {e}
  | "_" %prec prec_lower_than_arrow_fn { Pexpr_hole { loc_ = (i $sloc) ; kind = Incomplete } }
  | var_name=qual_ident_simple_expr { make_Pexpr_ident ~loc_:(i $sloc) { var_name; loc_ = i $sloc } }
  | c=constr { Parsing_syntax.Pexpr_constr {loc_ = (i $sloc); constr = c} }
  | func=simple_expr attr=apply_attr "(" args=list_commas(argument) ")" {
    Pexpr_apply { func; args; loc_ = i $sloc; attr }
  }
  | array=simple_expr  "[" index=expr "]" {
    Pexpr_array_get { array; index; loc_ = i $sloc }
  }
  | array=simple_expr  "[" start_index = option(expr) ":" end_index = option(expr) "]" {
    Pexpr_array_get_slice { array; start_index; end_index; loc_ = i $sloc; index_loc_ = (i ($startpos($2), $endpos($6))) }
  }
  | self=simple_expr meth=DOT_LIDENT attr=apply_attr "(" args=list_commas(argument) ")" {
    let method_name : Parsing_syntax.label =
      { label_name = meth; loc_ = i ($loc(meth)) }
    in
    Pexpr_dot_apply { self; method_name; args; return_self = false; attr; loc_ = (i $sloc) };
  }
  | self=simple_expr ".." meth=LIDENT attr=apply_attr "(" args=list_commas(argument) ")" {
    let method_name : Parsing_syntax.label =
      { label_name = meth; loc_ = i ($loc(meth)) }
    in
    Pexpr_dot_apply { self; method_name; args; return_self = true; attr; loc_ = (i $sloc) };
  }
  | record=simple_expr accessor=accessor %prec prec_field {
    Pexpr_field { record; accessor; loc_ = (i $sloc) }}
  | type_name=type_name "::" meth=LIDENT {
    let method_name: Parsing_syntax.label =
      { label_name = meth; loc_ = i ($loc(meth)) } in
    Pexpr_method { type_name; method_name; loc_ = i $sloc }
  }
  | "[" es = list_commas(spreadable_elem) "]" { (make_Pexpr_array ~loc_:(i $sloc) es) }
  | tuple_expr { $1 }

%inline label:
  name = LIDENT { { Parsing_syntax.label_name = name; loc_=(i $loc) } }
%inline accessor:
  | name = DOT_LIDENT {
    if name = "_"
    then Parsing_syntax.Newtype { loc_ = i $loc(name) }
    else Parsing_syntax.Label { label_name = name; loc_ = (i $loc) }
  }
  | index = DOT_INT { Parsing_syntax.Index { tuple_index = index; loc_ = (i $loc) } }
%inline binder:
  name = LIDENT { { Parsing_syntax.binder_name = name; loc_=(i $loc) } }
tvar_binder:
  | name = UIDENT {
      { Parsing_syntax.tvar_name = name; tvar_constraints = []; name_loc_=(i $loc) }
  }
  | name = UIDENT COLON constraints = separated_nonempty_list(PLUS, tvar_constraint) {
      { Parsing_syntax.tvar_name = name; tvar_constraints = constraints; name_loc_ = (i $loc(name)) }
  }
type_decl_binder:
  | name = UIDENT { { Parsing_syntax.tvar_name = Some name; loc_=(i $loc) } }
  | "_" { { Parsing_syntax.tvar_name = None; loc_ = (i $loc) } }
tvar_constraint:
  | qual_ident_ty { { Parsing_syntax.tvc_trait = $1; loc_ = i $sloc } }
%inline var:
  name = qual_ident { { Parsing_syntax.var_name = name; loc_=(i $loc) } }

type_name:
  | name = qual_ident_ty {
      { Parsing_syntax.name; is_object = false; loc_ = i $loc }
    }
  | "&" name = qual_ident_ty {
      { Parsing_syntax.name; is_object = true; loc_ = i $loc }
    }

multiline_string:
  | MULTILINE_STRING { Parsing_syntax.Multiline_string $1 }
  | MULTILINE_INTERP { Parsing_syntax.Multiline_interp (make_interps $1) }

atomic_expr:
  | simple_constant { make_Pexpr_constant ~loc_:(i $sloc) $1 }
  | non_empty_list(multiline_string) { 
      Parsing_syntax.Pexpr_multiline_string { loc_=(i $sloc); elems=($1) } 
    }
  | INTERP { (make_Pexpr_interp ~loc_:(i $sloc) ($1)) }

simple_constant:
  | TRUE { Parsing_syntax.Const_bool true }
  | FALSE { Parsing_syntax.Const_bool false }
  | BYTE { Parsing_syntax.Const_byte $1 }
  | BYTES { Parsing_syntax.Const_bytes $1 }
  | CHAR { Parsing_syntax.Const_char $1 }
  | INT { Parsing_util.make_int $1 }
  | DOUBLE { Parsing_util.make_double $1 }
  | FLOAT { Parsing_util.make_float $1 }
  | STRING { Parsing_syntax.Const_string $1 }

map_syntax_key:
  | simple_constant { $1 }
  | MINUS INT { Parsing_util.make_int ("-" ^ $2) }
  | MINUS DOUBLE { Parsing_util.make_double ("-" ^ $2) }
  | MINUS FLOAT { Parsing_util.make_float ("-" ^ $2) }

%inline loced_string:
  | STRING { {Rloc.v = $1; loc_ = i $sloc}}

%inline assignop:
  | AUGMENTED_ASSIGNMENT { {Parsing_syntax.var_name = Lident $1; loc_ = i $sloc} }

%inline infixop:
  | INFIX4
  | INFIX3
  | INFIX2
  | INFIX1 { {Parsing_syntax.var_name = Lident $1; loc_ = i $sloc} }
  | PLUS { {Parsing_syntax.var_name = Lident "+"; loc_ = i $sloc} }
  | MINUS  { {Parsing_syntax.var_name = Lident "-"; loc_ = i $sloc} }
  | AMPER { {Parsing_syntax.var_name = Lident "&"; loc_ = i $sloc} }
  | CARET { {Parsing_syntax.var_name = Lident "^"; loc_ = i $sloc} }
  | BAR { {Parsing_syntax.var_name = Lident "|"; loc_ = i $sloc} }
  | AMPERAMPER { {Parsing_syntax.var_name = Lident "&&"; loc_ = i $sloc} }
  | BARBAR { {Parsing_syntax.var_name = Lident "||"; loc_ =  i $sloc} }

optional_question:
  | "?" { Some(i $sloc) }
  | /* empty */ { None }

argument:
  (* label=expr *)
  | label=label is_question=optional_question "=" arg_value=expr {
    let arg_kind = 
      match is_question with
      | Some question_loc -> Parsing_syntax.Labelled_option { label; question_loc }
      | None -> Labelled {label}
    in
    { Parsing_syntax.arg_value; arg_kind }
  }
  (* expr *)
  | arg_value=expr { { Parsing_syntax.arg_value; arg_kind = Positional } }
  (* label~ *)
  | label=POST_LABEL {
    let label = { Parsing_syntax.label_name = label; loc_ = i $loc(label) } in
    let arg_value = Parsing_util.label_to_expr ~loc_:(Rloc.trim_last_char (i $loc(label))) label in
    { Parsing_syntax.arg_value; arg_kind = Labelled_pun {label} }
  }
  (* label~=expr. this is not recommended *)
  | label=POST_LABEL "=" arg_value=expr {
    let label = { Parsing_syntax.label_name = label; loc_ = i $loc(label) } in
    { Parsing_syntax.arg_value; arg_kind = Labelled {label} }
  }
  (* label? *)
  | id=LIDENT "?" {
    let loc_ = i $loc(id) in
    let label = { Parsing_syntax.label_name = id; loc_ } in
    let arg_value = make_Pexpr_ident ~loc_ { var_name = Lident id; loc_ } in
    { Parsing_syntax.arg_value; arg_kind = Labelled_option_pun { label; question_loc = i $loc($2) }}
  }
  

spreadable_elem:
  | expr=expr { Parsing_syntax.Elem_regular expr }
  | ".." expr=expr { Parsing_syntax.Elem_spread {expr; loc_=(i $sloc)} }

map_expr_elem:
  | key=map_syntax_key ":" expr=expr {
    Parsing_syntax.Map_expr_elem
      { key
      ; expr
      ; key_loc_ = i $loc(key)
      ; loc_ = i $sloc
      }
  }

pattern:
  | p=pattern "as" b=binder { (make_Ppat_alias ~loc_:(i $sloc) (p, b)) }
  | or_pattern { $1 }

or_pattern:
  | pat1=range_pattern "|" pat2=or_pattern { Parsing_syntax.Ppat_or {loc_=(i $sloc);  pat1 ; pat2 } }
  | range_pattern { $1 }

range_pattern:
  | lhs=simple_pattern "..<" rhs=simple_pattern {
      Parsing_syntax.Ppat_range { lhs; rhs; kind = Range_exclusive; loc_ = i $sloc }
    }
  | lhs=simple_pattern "..=" rhs=simple_pattern {
      Parsing_syntax.Ppat_range { lhs; rhs; kind = Range_inclusive; loc_ = i $sloc }
    }
  (* error recovery. The semantics are the same as `..<` *)
  | lhs=simple_pattern ".." rhs=simple_pattern {
      Parsing_syntax.Ppat_range { lhs; rhs; kind = Range_inclusive_missing_equal; loc_ = i $sloc }
    }
  | simple_pattern { $1 }

simple_pattern:
  | TRUE { (make_Ppat_constant  ~loc_:(i $sloc) (Const_bool true)) }
  | FALSE { (make_Ppat_constant ~loc_:(i $sloc) (Const_bool false)) }
  | CHAR { make_Ppat_constant ~loc_:(i $sloc) (Const_char $1) }
  | INT { (make_Ppat_constant ~loc_:(i $sloc) (Parsing_util.make_int $1)) }
  | BYTE { (make_Ppat_constant ~loc_:(i $sloc) (Const_byte $1)) }
  | DOUBLE { (make_Ppat_constant ~loc_:(i $sloc) (Const_double $1)) }
  | FLOAT { (make_Ppat_constant ~loc_:(i $sloc) (Parsing_util.make_float $1)) }
  | "-" INT { (make_Ppat_constant ~loc_:(i $sloc) (Parsing_util.make_int ("-" ^ $2))) }
  | "-" DOUBLE { (make_Ppat_constant ~loc_:(i $sloc) (Parsing_util.make_double ("-" ^ $2))) }
  | "-" FLOAT { (make_Ppat_constant ~loc_:(i $sloc) (Parsing_util.make_float ("-" ^ $2))) }
  | STRING { (make_Ppat_constant ~loc_:(i $sloc) (Const_string $1)) }
  | BYTES { (make_Ppat_constant ~loc_:(i $sloc) (Const_bytes $1)) }
  | REGEX_LITERAL { Parsing_syntax.Ppat_regex { lit = $1; loc_ = i $sloc } }
  | UNDERSCORE { Ppat_any {loc_ = i $sloc } }
  | b=binder  { Ppat_var b }
  | constr=constr ps=option(delimited("(", constr_pat_arguments, ")")){
    let (args, is_open) =
      match ps with
      | None -> (None, false)
      | Some (args, is_open) -> (Some args, is_open)
    in
    make_Ppat_constr ~loc_:(i $sloc) (constr, args, is_open)
  }
  (* bits constr pattern `i4(args)` 
     To disambiguate with pattern variable, the args list must be present.
  *)
  | name=binder ps=delimited("(", constr_pat_arguments_no_open, ")"){
    Ppat_special_constr { binder = name; args = ps; loc_ = i $sloc }
  }
  | "(" pattern ")" { $2 }
  | "(" p = pattern "," ps=non_empty_list_commas(pattern) ")"  {make_Ppat_tuple ~loc_:(i $sloc) (p::ps)}
  | "(" pat=pattern  ty=annot ")" { Parsing_syntax.Ppat_constraint {loc_=(i $sloc);  pat; ty } }
  | "[" pats=array_sub_patterns "]" { Ppat_array { loc_=(i $sloc); pats} }
  | "{" "}" { Parsing_syntax.Ppat_record { fields = []; is_closed = true; loc_ = i $sloc } }
  | "{" ".." option(",") "}" { Parsing_syntax.Ppat_record { fields = []; is_closed = false; loc_ = i $sloc } }
  | "{" p=non_empty_fields_pat "}" { let (fps, is_closed) = p in (Parsing_syntax.Ppat_record { fields=fps; is_closed; loc_=(i $sloc) }) }
  | "{" elems=non_empty_map_elems_pat "}" {
    let elems, is_closed = elems in
    Parsing_syntax.Ppat_map { elems; is_closed; loc_ = i $sloc }
  }

array_sub_pattern:
  | pattern { Parsing_syntax.Pattern($1) }
  | ".." s=STRING { Parsing_syntax.String_spread { str=s; loc_=i ($loc(s)) } }
  | ".." b=BYTES { Parsing_syntax.Bytes_spread { bytes=b; loc_=i ($loc(b)) } }
  | ".." b=UIDENT { Const_spread { binder = { binder_name = b; loc_=(i $loc(b)) }; pkg = None; loc_ = i $sloc } }
  | ".." pkg=PACKAGE_NAME b=DOT_UIDENT 
    { Const_spread { binder = { binder_name = b; loc_=(i $loc(b)) }; pkg = Some pkg; loc_ = i $sloc } }

dotdot_binder:
  | ".." b=binder { Parsing_syntax.Binder(b) }
  | ".." "_" { Parsing_syntax.Underscore }
  | ".." "as" b=binder { Parsing_syntax.Binder_as(b) }
  | ".." { Parsing_syntax.No_binder }

array_sub_patterns:
  | { Closed([]) }
  | p=array_sub_pattern { Closed([p]) }
  | p=array_sub_pattern "," rest=array_sub_patterns { 
    match rest with
    | Parsing_syntax.Closed(ps) -> Closed(p::ps)
    | Open(ps1, ps2, b) -> Open(p::ps1, ps2, b)
   }
  | b=dotdot_binder "," rest=non_empty_list_commas(array_sub_pattern) { Open([], rest, b) }
  // optional trailing comma
  | b=dotdot_binder ioption(",") { Open([], [], b) }

error_annotation:
  | "raise" {
    Parsing_syntax.Default_error_typ { loc_ = i $loc($1); is_old_syntax_ = false }
  }
  | "raise" ty=error_type {
    Parsing_syntax.Error_typ { ty; is_old_syntax_ = false }
  }
  | "noraise" { Noraise { loc_ = i $sloc } }
  | "raise" "?" {
    let fake_error : Parsing_syntax.typ =
      Ptype_name
        { constr_id = { lid = Lident "Error"; loc_ = i $sloc }
        ; tys = []
        ; loc_ = i $sloc
        }
    in
    Parsing_syntax.Maybe_error { ty = fake_error; is_old_syntax_ = false }
  }

return_type:
  | t=type_ { t, No_error_typ }
  | t1=simple_type "!" {
    t1, Default_error_typ { loc_ = i $loc($2); is_old_syntax_ = true }
  }
  | t1=simple_type "!" ty=error_type {
    t1, Error_typ { ty; is_old_syntax_ = true }
  }
  | ret=simple_type "?" err=error_type {
      ret, Maybe_error { ty = err; is_old_syntax_ = true }
  }
  | ret=simple_type err=error_annotation { ret, err }

func_return_type:
  | "->" ret=return_type {
    let return_type, error_type = ret in
    Some return_type, error_type
  }
  | err=error_annotation { None, err }
  | /* empty */ { None, No_error_typ }

error_type:
  | lid=qual_ident_ty {
    let loc_ = i $sloc in
    (Ptype_name { constr_id = { lid; loc_ }; tys = []; loc_ } : Parsing_syntax.typ)
  }
  | "_" {
    (Ptype_any { loc_ = i $sloc } : Parsing_syntax.typ)
  }

simple_type:
  | ty=simple_type "?" { make_Ptype_option ~loc_:(i $sloc) ~constr_loc:(i $loc($2)) ty }
  (* The tuple requires at least two elements, so non_empty_list_commas is used *)
  | "(" t=type_ "," ts=non_empty_list_commas(type_) ")" { (make_Ptype_tuple ~loc_:(i $sloc) (t::ts)) }
  | "(" t=type_ ")" { t }
  | id=qual_ident_ty_inline params=optional_type_arguments %prec prec_lower_than_as {
    Ptype_name {loc_ = (i $sloc) ;  constr_id = {lid=id;loc_=(i $loc(id))} ; tys =  params} }
  | "&" lid=qual_ident_ty {
    Ptype_object { lid; loc_ = i $loc(lid) }
  }
  | "_" { Parsing_syntax.Ptype_any {loc_ = i $sloc } }

type_:
  | ty=simple_type { ty }
  (* Arrow type input is not a tuple, it does not have arity restriction *)
  | is_async=is_async "(" t=type_ "," ts=ioption(non_empty_list_commas(type_)) ")" "->" rty=return_type {
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
        Parsing_syntax.Ptype_arrow { loc_=i($sloc); ty_arg=[t]; ty_res; ty_err; is_async }
      }


record_decl_field:
  | attrs=attributes field_vis=visibility mutflag=option("mut") name=LIDENT ":" ty=type_ {
    {Parsing_syntax.field_name = {Parsing_syntax.label = name; loc_ = i $loc(name)}; field_attrs=attrs; field_ty = ty; field_mut = mutflag <> None; field_vis; field_loc_ = i $sloc; field_doc = Docstring.empty }
  }

constructor_param:
  | mut=option("mut") ty=type_ {
    { Parsing_syntax.cparam_typ = ty; cparam_mut = [%p? Some _] mut; cparam_label = None }
  }
  (* mut label~ : Type *)
  | mut=option("mut") label_name=POST_LABEL ":" typ=type_ {
    let label : Parsing_syntax.label = { label_name; loc_ = Rloc.trim_last_char (i $loc(label_name)) } in
    { Parsing_syntax.cparam_typ = typ; cparam_mut = [%p? Some _] mut; cparam_label = Some label }
  }

enum_constructor:
  | attrs=attributes
    id=UIDENT
    constr_args=option(delimited("(", non_empty_list_commas(constructor_param), ")"))
    constr_tag=option(eq_int_tag) {
    let constr_name : Parsing_syntax.constr_name = { name = id; loc_ = i $loc(id) } in
    {Parsing_syntax.constr_name; constr_args; constr_tag; constr_attrs=attrs; constr_loc_ = i $sloc; constr_doc = Docstring.empty }
  }

%inline eq_int_tag:
  | "=" tag=INT { tag, i $loc(tag) }

record_defn:
  /* ending comma is required for single field {a,} for resolving the ambiguity between record punning {a} and block {a} */
  | l=label_pun "," x=list_commas_with_trailing_info(record_defn_single) {
      let (fs, trailing) = x in
      let trailing =
        if fs = [] || trailing then Parsing_syntax.Trailing_comma else Parsing_syntax.Trailing_none
      in
      (l::fs, trailing)
    }
  | l=labeled_expr comma=option(",") {
      ([l], if [%p? Some _] comma then Parsing_syntax.Trailing_comma else Parsing_syntax.Trailing_none)
    }
  /* rule out {r1: r1 r2} */
  | l=labeled_expr "," x=non_empty_list_commas_with_trailing_info(record_defn_single) {
      match x with
      | (fs, true) -> (l::fs, Parsing_syntax.Trailing_comma)
      | (fs, false) -> (l::fs, Parsing_syntax.Trailing_none)
    }

record_defn_single:
  | labeled_expr
  | label_pun {$1}

%inline labeled_expr:
  | l=label ":" e=expr {make_field_def ~loc_:(i $sloc) l e false}
%inline label_pun:
  | l=label {make_field_def ~loc_:(i $sloc) l (label_to_expr ~loc_:(i $sloc) l) true}

(* A field pattern list is a nonempty list of label-pattern pairs or punnings, optionally
   followed with an underscore, separated-or-terminated with commas. *)
non_empty_fields_pat:
  | fps=non_empty_list_commas(fields_pat_single) { fps, true }
  | fps=non_empty_list_commas_with_tail(fields_pat_single) ".." option(",") { fps, false }

fields_pat_single:
  | fpat_labeled_pattern
  | fpat_label_pun {$1}

%inline fpat_labeled_pattern:
  | l=label ":" p=pattern {make_field_pat ~loc_:(i $sloc) l p false}

%inline fpat_label_pun:
  | l=label {make_field_pat ~loc_:(i $sloc) l (label_to_pat ~loc_:(i $sloc) l) true}

non_empty_map_elems_pat:
  | non_empty_list_commas(map_elem_pat) { $1, true }
  | non_empty_list_commas_with_tail(map_elem_pat) ".." option(",") { $1, false }

%inline map_elem_pat:
  | key=map_syntax_key question=option("?") ":" pat=pattern {
    Parsing_syntax.Map_pat_elem
      { key
      ; pat
      ; match_absent = [%p? Some _] question
      ; key_loc_ = i $loc(key)
      ; loc_ = i $sloc
      }
  }

constr_pat_arguments:
  | constr_pat_argument option(",") { ([ $1 ], false) }
  | ".." option(",") { ([], true) }
  | arg=constr_pat_argument "," rest=constr_pat_arguments {
    let (args, is_open) = rest in
    (arg :: args, is_open)
  }

constr_pat_arguments_no_open:
  | constr_pat_argument option(",") { ([ $1 ]) }
  | arg=constr_pat_argument "," rest=constr_pat_arguments_no_open {
    (arg :: rest)
  }

constr_pat_argument:
  (* label=pattern *)
  | label=label "=" pat=pattern {
    Parsing_syntax.Constr_pat_arg { pat; kind = Labelled {label} }
  }
  (* label~=expr. this is not recommended *)
  | label_name=POST_LABEL "=" pat=pattern {
    let label : Parsing_syntax.label =
      { label_name; loc_ = i $loc(label_name) }
    in
    Parsing_syntax.Constr_pat_arg { pat; kind = Labelled {label} }
  }
  (* label~ *)
  | id=POST_LABEL {
    let loc_ = i $loc(id) in
    let label = { Parsing_syntax.label_name = id; loc_ } in
    let pat = Parsing_util.label_to_pat ~loc_:(Rloc.trim_last_char loc_) label in
    Parsing_syntax.Constr_pat_arg { pat; kind = Labelled_pun {label} }
  }
  (* pattern *)
  | pat=pattern { Parsing_syntax.Constr_pat_arg { pat; kind = Positional } }
