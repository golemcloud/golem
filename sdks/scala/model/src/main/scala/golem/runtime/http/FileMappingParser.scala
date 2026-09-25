// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
package golem.runtime.http

import java.nio.charset.StandardCharsets.UTF_8

/**
 * Compiles declaration syntax once; lookup consumes structural mappings, not
 * wildcard expressions.
 */
object FileMappingParser {
  def compile(mappings: List[(String, String)]): Either[String, List[FileMapping]] =
    mappings.foldLeft[Either[String, List[FileMapping]]](Right(Nil)) { case (result, (source, target)) =>
      for {
        prior   <- result
        mapping <- compileOne(source, target)
        _       <- if (prior.contains(mapping)) Left("duplicate-mapping") else Right(())
      } yield prior :+ mapping
    }

  private def compileOne(source: String, target: String): Either[String, FileMapping] = {
    val subtree = source.endsWith("/*")
    val path    = if (subtree) source.dropRight(2) match { case "" => "/"; case p => p } else source
    if (subtree && source.dropRight(2).endsWith("/")) Left("source-path")
    else if (path.contains('*')) Left("source-wildcard")
    else
      for {
        segments <- publicPath(path).left.map(_ => "source-path")
        root     <- if (subtree) {
                  if (!target.endsWith("/$1") || target.dropRight(3).contains('$')) Left("target-placeholder")
                  else if (target.dropRight(3).endsWith("/")) Left("target-path")
                  else Right(target.dropRight(3) match { case "" => "/"; case p => p })
                } else if (target.contains('$')) Left("target-placeholder")
                else Right(target)
        _ <- if (validFilesystemPath(root) && (subtree || root != "/")) Right(()) else Left("target-path")
      } yield if (subtree) FileMapping.Subtree(segments, root) else FileMapping.Exact(segments, root)
  }

  /**
   * Public literal path, split before exactly one strict percent/UTF-8 decoding
   * pass.
   */
  def publicPath(path: String): Either[String, List[String]] =
    if (!path.startsWith("/") || path.exists("$*?#".contains(_))) Left("source-path")
    else if (path == "/") Right(Nil)
    else
      path.drop(1).split("/", -1).toList.foldRight[Either[String, List[String]]](Right(Nil)) { (raw, tail) =>
        for { segment <- decodeSegment(raw); rest <- tail } yield segment :: rest
      }

  private def decodeSegment(raw: String): Either[String, String] = {
    val bytes = scala.collection.mutable.ArrayBuffer.empty[Byte]
    var i     = 0
    while (i < raw.length) {
      val c = raw.charAt(i)
      if (c == '%') {
        if (i + 2 >= raw.length) return Left("source-path")
        val high = Character.digit(raw.charAt(i + 1), 16)
        val low  = Character.digit(raw.charAt(i + 2), 16)
        if (high < 0 || low < 0 || raw.charAt(i + 1) > 127 || raw.charAt(i + 2) > 127) return Left("source-path")
        bytes += ((high << 4) | low).toByte
        i += 3
      } else {
        if (!(c.isLetterOrDigit && c <= 127) && !"-._~!$&'()+,;=:@".contains(c)) return Left("source-path")
        bytes += c.toByte
        i += 1
      }
    }
    val encoded = bytes.toArray
    val decoded = new String(encoded, UTF_8)
    if (!decoded.getBytes(UTF_8).sameElements(encoded) || !validSegment(decoded)) Left("source-path")
    else Right(decoded)
  }

  private[http] def validSegment(segment: String): Boolean =
    segment.nonEmpty && segment != "." && segment != ".." &&
      !segment.exists(c => c == '/' || c == '\\' || c < ' ' || c == 127) &&
      new String(segment.getBytes(UTF_8), UTF_8) == segment

  private[http] def validFilesystemPath(path: String): Boolean =
    !path.exists(Character.isISOControl) &&
      (path == "/" || (path.startsWith("/") && !path.contains('$') && path.drop(1).split("/", -1).forall(validSegment)))

  private[http] def validateCompiled(mappings: List[FileMapping]): Boolean =
    mappings.distinct.size == mappings.size && mappings.forall {
      case FileMapping.Exact(public, file)   => public.forall(validSegment) && file != "/" && validFilesystemPath(file)
      case FileMapping.Subtree(public, root) => public.forall(validSegment) && validFilesystemPath(root)
    }
}
