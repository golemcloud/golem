import { createContext, FC, ReactNode, useContext, useEffect, useState } from "react"

export const enum Release {
  R_1_1_0 = 1,
  R_1_1_1,
  R_1_2_2,
  R_1_2_2_1,
  R_1_2_4,
  R_1_3_0,
}

type ReleaseMeta = {
  json: string
  otherChanges?: ReactNode
}

const Releases: { [key in Release]: ReleaseMeta } = {
  [Release.R_1_1_0]: {
    json: "1.1.0",
  },
  [Release.R_1_1_1]: {
    json: "1.1.1",
  },
  [Release.R_1_2_2]: {
    json: "1.2.2",
    otherChanges: (
      <>
        <div>
          added enum value {code("library")} to
          {fieldLink({ path: "components.<component-name>[*].componentType" })}
        </div>
        <div>
          added enum values {code("wasm")} and {code("wasm-rpc-static")} to
          {fieldLink({ path: "dependencies.<component-name>.type" })}
        </div>
        <div>component and template schema property cleanups</div>
      </>
    ),
  },
  [Release.R_1_2_2_1]: {
    json: "1.2.2.1",
    otherChanges: (
      <>
        <div>external command schema property cleanups</div>
      </>
    ),
  },
  [Release.R_1_2_4]: {
    json: "1.2.4",
    otherChanges: <></>,
  },
  [Release.R_1_3_0]: {
    json: "1.3.0",
    otherChanges: (
      <>
        <div>
          added enum values {code("pretty-json")}, {code("pretty")} and {code("pretty-yaml")} to
          {fieldLink({ path: "profiles.<profile-name>.format" })}
        </div>
        <div>
          renamed {code("resetWorkers")} to {code("resetAgents")}
        </div>
        <div>deprecated wasm-rpc dependencies</div>
      </>
    ),
  },
}

export type FieldMeta = {
  path: string
  type?: string
  since?: Release
  deprecated?: Release
  noSpecialization?: boolean
}

type FieldProps = {
  meta: FieldMeta
  children: ReactNode
}

type FieldSpecialization = {
  pathPrefixMatch: string
  parentPrefix: string
  descriptionPrefix: string
}

// NOTE: order is important for matching
const FieldSpecializations: FieldSpecialization[] = [
  {
    pathPrefixMatch: "components.<component-name>.profiles.<profile-name>.",
    parentPrefix: "components.<component-name>.",
    descriptionPrefix: "Profile specific ",
  },
  {
    pathPrefixMatch: "templates.<template-name>.profiles.<profile-name>.",
    parentPrefix: "components.<component-name>.",
    descriptionPrefix: "Templated and profile specific ",
  },
  {
    pathPrefixMatch: "templates.<template-name>.",
    parentPrefix: "components.<component-name>.",
    descriptionPrefix: "Templated ",
  },
  {
    pathPrefixMatch: "components.<component-name>.customCommands.",
    parentPrefix: "customCommands.",
    descriptionPrefix: "Component specific ",
  },
  {
    pathPrefixMatch: "components.<component-name>.clean",
    parentPrefix: "clean",
    descriptionPrefix: "Component specific ",
  },
]

function fieldSpecializationDescription(path: string): ReactNode | undefined {
  const spec = FieldSpecializations.find(spec => {
    return path.startsWith(spec.pathPrefixMatch)
  })

  if (!spec) {
    return undefined
  }

  const parentPath = `${spec.parentPrefix}${path.substring(spec.pathPrefixMatch.length)}`

  return (
    <p className="nx-mt-6 nx-leading-7">
      <em>
        <strong>{spec.descriptionPrefix}</strong>
      </em>
      {fieldLink({ path: parentPath })}.
    </p>
  )
}

function relatedFieldDescription(path: string): ReactNode | undefined {
  const relatedSpecs = FieldSpecializations.filter(spec => {
    const pathContainsProfile = path.indexOf("profiles") !== -1
    const parentContainsProfile = spec.parentPrefix.indexOf("profiles") !== -1
    return pathContainsProfile == parentContainsProfile && path.startsWith(spec.parentPrefix)
  })

  if (relatedSpecs.length === 0) {
    return undefined
  }

  return (
    <div className="nx-text-xs">
      <p className="nx-mt-6 nx-leading-6">
        <em>
          <strong>Related fields:</strong>
        </em>
      </p>
      <ul className="nx-list-disc first:nx-mt-0 ltr:nx-ml-6 rtl:nx-mr-6">
        {relatedSpecs.map(spec => {
          const relatedPath = `${spec.pathPrefixMatch}${path.substring(spec.parentPrefix.length)}`
          return (
            <li className="nx-my-2" key={relatedPath}>
              {fieldLink({ path: relatedPath })}
            </li>
          )
        })}
      </ul>
    </div>
  )
}

const FieldsContext = createContext<{
  addField: (release: Release, path: string, id: string) => void
  getFields: () => Record<Release, Record<string, string>>
} | null>(null)

export const Field: FC<FieldProps> = ({
  meta: { path, since, deprecated, noSpecialization },
  children: children,
}) => {
  const since_release = release(since)
  const deprecated_release = deprecated ? release(deprecated) : undefined
  const id = fieldPathToId(path)
  const specializationDescription = !noSpecialization && fieldSpecializationDescription(path)
  const relatedDescription = !noSpecialization && relatedFieldDescription(path)

  const fieldContext = useContext(FieldsContext)
  useEffect(() => {
    fieldContext?.addField(since_release, path, id)
  })

  let headerClassName = "nx-tracking-tight nx-text-slate-900 dark:nx-text-slate-100 nx-mt-8"
  if (deprecated) {
    headerClassName += " line-through"
  } else {
    headerClassName += " nx-font-semibold"
  }

  return (
    <>
      <h5 className={headerClassName}>
        <code
          className="nx-border-black nx-border-opacity-[0.04] nx-bg-opacity-[0.03] nx-bg-black nx-break-words nx-rounded-md nx-border nx-py-0.5 nx-px-[.25em] nx-text-[.9em] dark:nx-border-white/10 dark:nx-bg-white/10"
          dir="ltr"
        >
          {path}
        </code>
        <a
          href={`#${id}`}
          id={id}
          className="subheading-anchor"
          aria-label="Permalink for this section"
        ></a>
      </h5>
      <div className="nx-bg-primary-700/5 dark:nx-bg-primary-300/10 nx-rounded-lg nx-px-2 nx-py-2">
        {availableAndDeprecatedSince(since_release, deprecated_release, "nx-flex nx-justify-end")}
        <hr className="nx-opacity-50" />
        {specializationDescription && specializationDescription}
        {children}
        {relatedDescription && relatedDescription}
      </div>
    </>
  )
}

type ExampleProps = {
  children: ReactNode
}

export const Example: FC<ExampleProps> = ({ children }) => {
  return (
    <>
      <div className="nx-font-semibold nx-mt-6">Example usage:</div>
      {children}
    </>
  )
}

type EnumValuesProps = {
  children: ReactNode
}

export const EnumValues: FC<EnumValuesProps> = ({ children }) => {
  return <ul className="nx-list-disc first:nx-mt-0 ltr:nx-ml-6 rtl:nx-mr-6">{children}</ul>
}

type EnumValueMeta = {
  value: string
  isDefault?: boolean
  since?: Release
  deprecated?: Release
}

type EnumValueProps = {
  meta: EnumValueMeta
  children: ReactNode
}

export const EnumValue: FC<EnumValueProps> = ({
  meta: { value, isDefault, since, deprecated },
  children,
}: EnumValueProps) => {
  let codeClassName =
    "nx-border-black nx-border-opacity-[0.04] nx-bg-opacity-[0.03] nx-bg-black nx-break-words nx-rounded-md nx-border nx-py-0.5 nx-px-[.25em] nx-text-[.9em] dark:nx-border-white/10 dark:nx-bg-white/10"
  if (deprecated) {
    codeClassName += " line-through"
  } else {
    codeClassName += " nx-font-semibold"
  }
  return (
    <li className="nx-my-2">
      <div>
        <code className={codeClassName} dir="ltr">
          {value}
        </code>
        {isDefault && <span className="nx-italic nx-px-3">(default value)</span>}
        {availableAndDeprecatedSince(release(since), deprecated ? release(deprecated) : undefined)}
        <div>{children}</div>
      </div>
    </li>
  )
}

type FieldsProps = {
  children: ReactNode
}

export const Fields: FC<FieldsProps> = ({ children }) => {
  const [fields, setFields] = useState<Record<Release, Record<string, string>>>({
    [Release.R_1_1_0]: {},
    [Release.R_1_1_1]: {},
    [Release.R_1_2_2]: {},
    [Release.R_1_2_2_1]: {},
    [Release.R_1_2_4]: {},
    [Release.R_1_3_0]: {},
  })

  const addField = (relase: Release, path: string, id: string) => {
    setFields(prev => {
      prev[relase][path] = id
      return prev
    })
  }

  const getFields = () => {
    return fields
  }

  return <FieldsContext.Provider value={{ addField, getFields }}>{children}</FieldsContext.Provider>
}

export const FieldReleases: FC = ({}) => {
  const fields = useContext(FieldsContext)?.getFields()
  if (!fields) {
    throw "Missing fields"
  }
  return (
    <>
      <div className="nx-text-xs nx-py-4">
        {(Object.keys(fields) as unknown as Release[]).map(release => {
          return (
            <div key={release}>
              <div className="nx-py-4">{availableAndDeprecatedSince(release, undefined)}</div>
              {Object.keys(fields[release])
                .sort()
                .map(path => {
                  const id = fields[release][path]
                  return <div key={id}>{fieldLink({ path: path, id: id })}</div>
                })}
              {Releases[release].otherChanges && Releases[release].otherChanges}
              <hr className="nx-my-4" />
            </div>
          )
        })}
      </div>
    </>
  )
}

function release(since: Release | undefined): Release {
  if (!since) {
    since = Release.R_1_1_0
  }
  return since
}

function availableAndDeprecatedSince(
  since: Release,
  deprecated: Release | undefined,
  divClassExt?: string
) {
  const release = Releases[since]
  let className = "nx-text-xs nx-py-1"
  if (divClassExt) {
    className += " "
    className += divClassExt
  }

  let deprecatedBlock = <></>
  if (deprecated) {
    const release = Releases[deprecated]
    deprecatedBlock = (
      <>
        <span className="nx-px-1 nx-italic nx-font-bold">deprecated since</span>
        <span className="nx-font-bold">{release.json}</span>
        <span className="nx-select-none nx-px-1">|</span>
      </>
    )
  }

  return (
    <div className={className}>
      {deprecatedBlock}
      <span className="nx-px-1 nx-italic">available since</span>
      <span className="nx-font-bold">{release.json}</span>
    </div>
  )
}

function fieldPathToId(path: string): string {
  return `fields_${path.replaceAll(/[<>\[\].*-]/g, "_")}`
}

function code(text: string): ReactNode {
  return (
    <code
      className="nx-break-words nx-py-0.5 nx-px-[.25em] nx-text-[.9em] dark:nx-border-white/10 dark:nx-bg-white/10"
      dir="ltr"
    >
      {text}
    </code>
  )
}

type FieldLinkProps = {
  path: string
  id?: string
}

export function fieldLink({ path, id }: FieldLinkProps): ReactNode {
  return (
    <a href={`#${id || fieldPathToId(path)}`}>
      <code
        className="nx-underline nx-break-words nx-py-0.5 nx-px-[.25em] nx-text-[.9em] dark:nx-border-white/10 dark:nx-bg-white/10"
        dir="ltr"
      >
        {path}
      </code>
    </a>
  )
}

export const FieldLink: FC<FieldLinkProps> = props => {
  return fieldLink(props)
}
