import SwaggerParser from "@apidevtools/swagger-parser"
import type { JSONSchema7 } from "json-schema"
import { writeFile } from "fs/promises"
import { OpenAPIV3 } from "openapi-types"
import OpenAPISampler from "openapi-sampler"

// The canonical OpenAPI spec lives in the golem repo root at
// openapi/golem-service.yaml and is regenerated from the Rust services
// by `cargo make generate-openapi`. This script reads from there directly
// (one level up from docs/) and writes per-tag MDX into
// src/content/<version>/rest-api/. The in-tree OpenAPI spec is the source of
// truth for the upcoming release, so by default we target the unreleased
// `next` version; pass `--version <slug>` to write into another version's
// directory (e.g., to backport a fix to an already-released version).
const CLOUD_SPEC_SRC = "../openapi/golem-service.yaml"
const CONTENT_ROOT = "./src/content"
const DEFAULT_TARGET_VERSION = "next"

main().catch(e => {
  console.error("Failed to update API Docs", e)
  process.exit(1)
})

function parseVersionArg(argv: string[]): string {
  const i = argv.indexOf("--version")
  if (i === -1) return DEFAULT_TARGET_VERSION
  const v = argv[i + 1]
  if (!v) throw new Error("--version requires a value (e.g. --version next)")
  return v
}

async function main() {
  const version = parseVersionArg(process.argv.slice(2))
  const target = `${CONTENT_ROOT}/${version}/rest-api`
  console.log(
    `Updating REST API docs (version=${version}) from local OpenAPI spec at: ${CLOUD_SPEC_SRC}`
  )
  console.log(`Output directory: ${target}`)
  await writeOpenApiDocs(target, CLOUD_SPEC_SRC)
}

async function writeOpenApiDocs(target: string, openapiSpec: string) {
  const api = (await SwaggerParser.parse(openapiSpec)) as OpenAPIV3.Document

  const tags = api.tags

  if (!tags) {
    throw new Error("No tags")
  }

  const apiItems = extractApiItems(api)

  const grouped = groupBy(apiItems, item => {
    const tagId = item.operation.tags?.find(tag => tag !== "RegistryService")
    if (!tagId) {
      throw new Error(`Operation Missing Tag: ${JSON.stringify(item)}}`)
    }

    const foundTag = tags.find(tag => tag.name === tagId)

    if (!foundTag) {
      throw new Error(`Invalid Tag ${tagId}`)
    }

    return foundTag
  })

  const markdown = Array.from(grouped.entries())
    .filter(([tag, _]) => {
      return tag.name !== "RegistryService" && tag.name !== "HealthCheck" && tag.name !== "Reports"
    })
    .map(([tag, items]) => [tag, convertToMarkdown(api, tag, items)] as const)

  await Promise.all(
    markdown.map(([tag, content]) => {
      const fileName = pascalToKebab(tag.name)
      const path = `${target}/${fileName}.mdx`
      writeFile(path, content)
    })
  )

  console.log("Finished Writing docs")
}

type ApiItem = {
  path: string
  method: string
  operation: OpenAPIV3.OperationObject
}

function extractApiItems(api: OpenAPIV3.Document): ApiItem[] {
  return Object.entries(api.paths)
    .filter(([_, pathItem]) => pathItem !== undefined)
    .flatMap(([path, pathItem]) =>
      Object.entries(pathItem!).map(([method, operation]) => {
        return {
          path,
          method,
          operation: operation as OpenAPIV3.OperationObject,
        }
      })
    )
}

function convertToMarkdown(
  api: OpenAPIV3.Document,
  tag: OpenAPIV3.TagObject,
  apiItems: ApiItem[]
): string {
  const title = pascalToSpace(tag.name)

  const itemsMarkdown = apiItems.map(i => convertItemToMarkdown(api, i)).join("\n\n")

  const errors = makeRequestError(api, apiItems[0].operation)

  const errorTitle = errors ? `## ${title} API Errors` : ""
  const errorsContent = errors ? errors : ""
  const errorSection = errors ? `${errorTitle}\n${errorsContent}` : ""

  return [`# ${title} API`, tag.description, itemsMarkdown, errorSection].join("\n")
}

function convertItemToMarkdown(
  api: OpenAPIV3.Document,
  { path, method, operation }: ApiItem
): string {
  const overviewTable = makeMarkdownTable({
    headers: ["Path", "Method", "Protected"],
    rows: [[`\`${path}\``, method.toUpperCase(), operation.security === undefined ? "No" : "Yes"]],
  })

  const queryParams = (operation.parameters?.filter(
    param => !!param && "in" in param && param.in === "query"
  ) ?? []) as OpenAPIV3.ParameterObject[]

  const queryParamsTable = makeQueryParamTable(queryParams)

  const explanation = operation.description === undefined ? "" : operation.description

  const requestBody = makeRequestBody(api, operation)

  const response = makeResponseType(api, operation)

  return [
    `## ${operation.summary}`,
    overviewTable,
    "",
    explanation,
    "",
    queryParamsTable,
    "",
    requestBody,
    "",
    response,
  ].join("\n")
}

type MdTable = {
  headers: string[]
  rows: string[][]
}

function makeMarkdownTable(table: MdTable) {
  const header = table.headers.join("|")
  const headerLine = table.headers.map(_ => "---").join("|")
  const rows = table.rows.map(row => row.join("|")).join("\n")

  return `${header}\n${headerLine}\n${rows}`
}

function makeQueryParamTable(queryParams: OpenAPIV3.ParameterObject[]) {
  if (queryParams.length > 0) {
    const rows = queryParams.map(param => {
      const schema = param?.schema
      if (!schema) {
        throw new Error(`No schema ${JSON.stringify(param)}}`)
      }

      const type = (
        "type" in schema ? schema.type : "$ref" in schema ? schema.$ref : schema
      ) as string

      const description = param.description ?? "-"
      const required = param.required ? "Yes" : "No"

      return [param.name, type, required, description]
    })
    const table = makeMarkdownTable({
      headers: ["Name", "Type", "Required", "Description"],
      rows,
    })

    return `**Query Parameters**\n\n${table}`
  } else {
    return ""
  }
}

function makeResponseType(api: OpenAPIV3.Document, operation: OpenAPIV3.OperationObject) {
  const response = (() => {
    const successResponse =
      operation.responses["200"] || operation.responses["204"] || operation.responses["101"]

    // No 200/204/101 declared (e.g. an OAuth callback that only responds with
    // a 302 redirect, or an endpoint with no documented success body), and
    // 204/200-with-no-content cases both fall through here.
    if (!successResponse || !("content" in successResponse)) {
      return { content: undefined }
    }

    return successResponse
  })()

  const jsonResponse = response.content?.["application/json; charset=utf-8"]?.schema
  const octetStreamResponse = response.content?.["application/octet-stream"]

  if (jsonResponse !== undefined) {
    const sample = OpenAPISampler.sample(jsonResponse as JSONSchema7, undefined, api)
    return [
      `**Example Response JSON**`,
      "",
      "```json copy",
      JSON.stringify(sample, null, 2),
      "```",
    ].join("\n")
  } else if (octetStreamResponse !== undefined) {
    return ["**Response Body:**", "`WASM Binary File`"].join(" ")
  }
}

function makeRequestBody(api: OpenAPIV3.Document, operation: OpenAPIV3.OperationObject) {
  const requestBody = operation.requestBody
  if (requestBody === undefined) {
    return ""
  } else {
    const content = "content" in requestBody ? requestBody?.content : undefined
    if (!!content) {
      const jsonSchema = content["application/json; charset=utf-8"]?.schema
      const octetStreamSchema = content["application/octet-stream"]?.schema
      const formSchema = content["multipart/form-data"]?.schema

      if (jsonSchema !== undefined) {
        const sample = OpenAPISampler.sample(jsonSchema as JSONSchema7, undefined, api)
        return [
          `**Example Request JSON**`,
          "```json copy",
          JSON.stringify(sample, null, 2),
          "```",
        ].join("\n")
      } else if (octetStreamSchema !== undefined) {
        return (
          [
            "**Request Body**: `WASM Binary File`",
            "> Make sure to include `Content-Type: application/octet-stream` Header",
          ].join("\n") + "\n"
        )
      } else if (formSchema !== undefined) {
        if ("$ref" in formSchema) {
          throw new Error("Form Schema is a ref")
        }

        const properties = formSchema.properties

        if (!properties) {
          throw new Error("No form properties")
        }

        const propertyString = Object.entries(properties).map(([name, prop]) => {
          if ("$ref" in prop) {
            const ref = prop.$ref.replace("#/components/schemas/", "")
            const component = api.components!.schemas![ref] as OpenAPIV3.SchemaObject
            if (!component) {
              throw new Error(`No component ${ref}`)
            }
            const example = OpenAPISampler.sample(component as JSONSchema7, undefined, api)
            return [
              `**Field \`${name}\`**: JSON`,
              "```json copy",
              JSON.stringify(example, null, 2),
              "```",
            ].join("\n")
          } else {
            return `**Field \`${name}\`**: ${prop.type} ${prop.format}`
          }
        })

        return [
          "**Request Form**: `multipart/form-data`",
          "> Make sure to include `Content-Type: multipart/form-data` Header",
          propertyString.join("\n\n"),
        ].join("\n\n")
      }
    }
  }
}

function makeRequestError(api: OpenAPIV3.Document, operation: OpenAPIV3.OperationObject) {
  const errorResponses = Object.entries(operation.responses)
    .filter(([code, _]) => code !== "200")
    .map(([status, response]) => {
      if ("content" in response) {
        const schema = response.content?.["application/json; charset=utf-8"]?.schema
        if (schema) {
          return { status, desc: response.description, schema }
        }
      }
      return undefined
    })
    .filter(resp => resp !== undefined)
    .map(resp => resp!)
    .map(({ status, desc, schema }) => {
      const sample = OpenAPISampler.sample(schema as JSONSchema7, undefined, api)

      const jsonRendered = ["`", JSON.stringify(sample), "`"].join("")

      const shortDesc = desc?.split("\n").join(" ")

      return [status, shortDesc, jsonRendered]
    })

  if (errorResponses.length === 0) {
    return undefined
  }

  return makeMarkdownTable({
    headers: ["Status Code", "Description", "Body"],
    rows: errorResponses,
  })
}

function groupBy<T, K>(items: T[], keySelector: (item: T) => K): Map<K, [T]> {
  const map = new Map<K, [T]>()
  items.forEach(item => {
    const key = keySelector(item)
    const existing = map.get(key)
    if (existing) {
      existing.push(item)
    } else {
      map.set(key, [item])
    }
  })

  return map
}

function pascalToKebab(str: string) {
  return str.replace(/([a-z0-9])(?=[A-Z])/g, "$1-").toLowerCase()
}

function pascalToSpace(str: string) {
  return str.replace(/([a-z])([A-Z])/g, "$1 $2")
}
