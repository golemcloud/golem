// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
package golem.runtime.macros

import golem.runtime.http.{FileMapping, FileMappingParser, HttpMountDetails, PathSegment}
import scala.quoted.*

private[macros] object HttpDeclarationMacro {
  def has(using Quotes)(symbol: quotes.reflect.Symbol, name: String): Boolean =
    symbol.annotations.exists(_.tpe.typeSymbol.fullName == s"golem.runtime.annotations.$name")

  def isRouter(using Quotes)(symbol: quotes.reflect.Symbol): Boolean = has(symbol, "httpRouter")

  def argument(using
    Quotes
  )(symbol: quotes.reflect.Symbol, annotation: String, name: String, index: Int): Option[quotes.reflect.Term] = {
    import quotes.reflect.*
    symbol.annotations.find(_.tpe.typeSymbol.fullName == s"golem.runtime.annotations.$annotation").flatMap {
      case Apply(_, args) =>
        args.collectFirst { case NamedArg(`name`, value) => value }.orElse(args.lift(index).collect {
          case value if (value match { case NamedArg(_, _) => false; case _ => true }) => value
        })
      case _ => None
    }
  }

  def string(using Quotes)(symbol: quotes.reflect.Symbol, name: String, index: Int): String = {
    import quotes.reflect.*
    argument(symbol, "httpRouter", name, index) match {
      case Some(Literal(StringConstant(value))) if value.nonEmpty => value
      case _                                                      => report.errorAndAbort(s"@httpRouter requires a nonempty literal $name")
    }
  }

  private def elements(using Quotes)(term: quotes.reflect.Term): List[quotes.reflect.Term] = {
    import quotes.reflect.*
    term match {
      case Inlined(_, _, value)                                                      => elements(value)
      case Typed(value, _)                                                           => elements(value)
      case Repeated(values, _)                                                       => values
      case Apply(inner @ Apply(_, _), _)                                             => elements(inner)
      case Apply(_, List(value @ Typed(Repeated(_, _), _)))                          => elements(value)
      case Apply(function, Nil) if function.symbol.fullName == "scala.Array.apply"   => Nil
      case value if value.symbol.name.contains("$default$")                          => Nil
      case TypeApply(function, _) if function.symbol.fullName == "scala.Array.empty" => Nil
      case _                                                                         => report.errorAndAbort("HTTP declarations require literal Array(...) arguments")
    }
  }

  def mappings(using
    Quotes
  )(symbol: quotes.reflect.Symbol, annotation: String, name: String, index: Int): Expr[List[FileMapping]] = {
    val compiled = mappingValues(symbol, annotation, name, index)
    Expr.ofList(compiled.map {
      case FileMapping.Exact(public, file) =>
        '{ FileMapping.Exact(${ Expr.ofList(public.map(Expr(_))) }, ${ Expr(file) }) }
      case FileMapping.Subtree(public, root) =>
        '{ FileMapping.Subtree(${ Expr.ofList(public.map(Expr(_))) }, ${ Expr(root) }) }
    })
  }

  def mappingValues(using
    Quotes
  )(symbol: quotes.reflect.Symbol, annotation: String, name: String, index: Int): List[FileMapping] = {
    import quotes.reflect.*
    val pairs = argument(symbol, annotation, name, index).toList.flatMap(elements).map {
      case Apply(_, List(Literal(StringConstant(source)), Literal(StringConstant(target)))) => (source, target)
      case value                                                                            => report.errorAndAbort(s"$name requires literal (route, path) pairs: ${value.show}")
    }
    FileMappingParser.compile(pairs).fold(error => report.errorAndAbort(s"$name: $error"), identity)
  }

  def exposesFiles(using Quotes)(symbol: quotes.reflect.Symbol): Boolean =
    argument(symbol, "agentDefinition", "exposeFiles", 8).exists(elements(_).nonEmpty)

  def routerMount(using Quotes)(symbol: quotes.reflect.Symbol): Expr[Option[HttpMountDetails]] = {
    import quotes.reflect.*
    val path = FileMappingParser
      .publicPath(string(symbol, "mount", 1))
      .fold(error => report.errorAndAbort(s"router-mount: $error"), identity)
    val pathExpr = Expr.ofList(path.map(s => '{ PathSegment.Literal(${ Expr(s) }) }))
    val static   = mappings(symbol, "httpRouter", "staticBindings", 2)
    val auth     = argument(symbol, "httpRouter", "auth", 3) match {
      case Some(Literal(BooleanConstant(value)))                  => value
      case Some(value) if value.symbol.name.contains("$default$") => false
      case None                                                   => false
      case _                                                      => report.errorAndAbort("router auth must be a literal boolean")
    }
    val cors = argument(symbol, "httpRouter", "cors", 4).toList.flatMap(elements).map {
      case Literal(StringConstant(value)) => value
      case _                              => report.errorAndAbort("router cors must contain literal strings")
    }
    val providers = symbol.methodMembers.filter(has(_, "openApiProvider"))
    if (providers.size > 1) report.errorAndAbort("router-method-role: at most one OpenAPI provider is allowed")
    val provider = providers.headOption match {
      case Some(method) => '{ Some(${ Expr(method.name) }) }
      case None         => '{ None }
    }
    '{
      Some(
        HttpMountDetails(
          $pathExpr,
          ${ Expr(auth) },
          false,
          ${ Expr.ofList(cors.map(Expr(_))) },
          Nil,
          $static,
          Nil,
          $provider
        )
      )
    }
  }
}
