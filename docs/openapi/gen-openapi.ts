import SwaggerParser from "@apidevtools/swagger-parser"
import type { JSONSchema7 } from "json-schema"
import { writeFile } from "fs/promises"
import { OpenAPIV3 } from "openapi-types"
import OpenAPISampler from "openapi-sampler"

const CLOUD_SPEC_SRC = "./openapi/cloud-spec.yaml"
const CLOUD_GEN_PATH = "./src/pages/docs/rest-api/cloud-rest-api"
const OSS_SPEC_SRC = "./openapi/oss-spec.yaml"
const OSS_GEN_PATH = "./src/pages/docs/rest-api/oss-rest-api"

main().catch(e => console.error("Failed to update API Docs", e))

// Two options are available:
// --production - will generate the docs for the Production Golem API, and update the local yaml file.
// --local - will generate the docs from local yaml files.
async function main() {
  const args = process.argv.slice(2)

  if (args.length !== 1 || !["--prod", "--dev", "--local"].includes(args[0])) {
    throw new Error("Invalid args: must be one of --prod --dev --local")
  }

  const [mode] = args

  if (mode === "--local") {
    console.log("Updating REST API docs from local OpenAPI spec at:", [
      CLOUD_SPEC_SRC,
      OSS_SPEC_SRC,
    ])
    await writeOpenApiDocs(CLOUD_GEN_PATH, CLOUD_SPEC_SRC)
    await writeOpenApiDocs(OSS_GEN_PATH, OSS_SPEC_SRC)
  } else {
    let specUrl =
      mode === "--prod"
        ? "https://release.api.golem.cloud/specs"
        : "https://release.dev-api.golem.cloud/specs"

    console.log("Updating REST API docs from OpenAPI spec at:", specUrl)
    await writeOpenApiDocs(CLOUD_GEN_PATH, specUrl)
    const response = await fetch(specUrl)
    if (!response.ok) {
      throw new Error(`Error fetching data: ${response.status}`)
    }

    const textContent = await response.text()
    await writeFile(CLOUD_SPEC_SRC, textContent)
  }
}

async function writeOpenApiDocs(target: string, openapiSpec: string) {
  const api = (await SwaggerParser.parse(openapiSpec)) as OpenAPIV3.Document

  const tags = api.tags

  if (!tags) {
    throw new Error("No tags")
  }

  const apiItems = extractApiItems(api)

  const grouped = groupBy(apiItems, item => {
    const tagId = item.operation.tags?.at(0)
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
    .filter(([tag, _]) => tag.name !== "HealthCheck")
    .filter(([tag, _]) => tag.name !== "Grant")
    .filter(([tag, _]) => tag.name !== "AccountSummary")
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
    const successResponse = operation.responses["200"]
    if (!successResponse || !("content" in successResponse)) {
      throw new Error("No Success Response")
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
