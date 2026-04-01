/*
 * Copyright 2024-2026 John A. De Goes and the ZIO Contributors
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

package golem.runtime.annotations

import java.lang.annotation.{ElementType, Retention, RetentionPolicy, Target}
import scala.annotation.StaticAnnotation

/** Human-readable description for an agent, method, or type. */
@Retention(RetentionPolicy.RUNTIME)
@Target(Array(ElementType.TYPE, ElementType.METHOD, ElementType.FIELD, ElementType.PARAMETER))
final class description(val value: String) extends StaticAnnotation

/** Optional prompt hint for LLM-driven invocations. */
@Retention(RetentionPolicy.RUNTIME)
@Target(Array(ElementType.METHOD))
final class prompt(val value: String) extends StaticAnnotation

/** Marks a class/object as an agent implementation. */
@Retention(RetentionPolicy.RUNTIME)
@Target(Array(ElementType.TYPE))
final class agentImplementation() extends StaticAnnotation

/**
 * Explicitly marks a class inside an agent trait as the id schema. Optional —
 * by default, a class named `Id` is used automatically. Use this annotation to
 * designate a differently-named class as the id schema.
 */
@Retention(RetentionPolicy.RUNTIME)
@Target(Array(ElementType.TYPE))
final class id() extends StaticAnnotation

/**
 * Overrides the language code used by multimodal/unstructured text derivation.
 */
@Retention(RetentionPolicy.RUNTIME)
@Target(Array(ElementType.TYPE, ElementType.METHOD, ElementType.FIELD, ElementType.PARAMETER))
final class languageCode(val value: String) extends StaticAnnotation

/**
 * Overrides the MIME type used by multimodal/unstructured binary derivation.
 */
@Retention(RetentionPolicy.RUNTIME)
@Target(Array(ElementType.TYPE, ElementType.METHOD, ElementType.FIELD, ElementType.PARAMETER))
final class mimeType(val value: String) extends StaticAnnotation
