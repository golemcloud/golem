import { createContext, FC, ReactNode, useContext, useEffect, useState } from "react"

export const enum Release {
  R_1_1_0 = 1,
  R_1_1_1,
  R_1_1_2,
}

type ReleaseMeta = {
  json: string
  ossCli: string
  cloudCli: string
  otherChanges?: ReactNode
}

const Releases: { [key in Release]: ReleaseMeta } = {
  [Release.R_1_1_0]: {
    json: "1.1.0",
    ossCli: "1.1.0",
    cloudCli: "1.1.0",
  },
  [Release.R_1_1_1]: {
    json: "1.1.1",
    ossCli: "1.1.12",
    cloudCli: "1.1.1",
  },
  [Release.R_1_1_2]: {
    json: "1.1.2",
    ossCli: "1.1.12",
    cloudCli: "1.1.2",
    otherChanges: (
      <>
        <div>
          added enum value {code("static-wasm-rpc")} to
          {fieldLink({ path: "dependencies.<component-name>[*].type" })}
        </div>
      </>
    ),
  },
}

export type FieldMeta = {
  path: string
  type?: string
  since?: Release
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
  meta: { path, since, noSpecialization },
  children: children,
}) => {
  const since_release = release(since)
  const id = fieldPathToId(path)
  const specializationDescription = !noSpecialization && fieldSpecializationDescription(path)
  const relatedDescription = !noSpecialization && relatedFieldDescription(path)

  const fieldContext = useContext(FieldsContext)
  useEffect(() => {
    fieldContext?.addField(since_release, path, id)
  })

  return (
    <>
      <h5 className="nx-font-semibold nx-tracking-tight nx-text-slate-900 dark:nx-text-slate-100 nx-mt-8 nx-mb-2">
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
        {availableSince(since_release, "nx-flex nx-justify-end")}
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
}

type EnumValueProps = {
  meta: EnumValueMeta
  children: ReactNode
}

export const EnumValue: FC<EnumValueProps> = ({
  meta: { value, isDefault, since },
  children,
}: EnumValueProps) => {
  return (
    <li className="nx-my-2">
      <div>
        <code
          className="nx-border-black nx-border-opacity-[0.04] nx-bg-opacity-[0.03] nx-bg-black nx-break-words nx-rounded-md nx-border nx-py-0.5 nx-px-[.25em] nx-text-[.9em] dark:nx-border-white/10 dark:nx-bg-white/10 nx-font-semibold"
          dir="ltr"
        >
          {value}
        </code>
        {isDefault && <span className="nx-italic nx-px-3">(default value)</span>}
        {availableSince(release(since))}
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
    [Release.R_1_1_2]: {},
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
              <div className="nx-py-4">{availableSince(release)}</div>
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

function availableSince(since: Release, divClassExt?: string) {
  const release = Releases[since]
  let className = "nx-text-xs nx-py-1"
  if (divClassExt) {
    className += " "
    className += divClassExt
  }
  return (
    <div className={className}>
      <span className="nx-px-1 nx-italic">available since</span>
      <span className="nx-font-bold nx-px-1">JSON Schema:</span>
      <span className="nx-font-bold">{release.json}</span>
      <span className="nx-select-none nx-px-1">|</span>
      <span className="nx-font-bold nx-px-1">OSS CLI: </span>
      <span className="nx-font-bold">{release.ossCli}</span>
      <span className="nx-select-none nx-px-1">|</span>
      <span className="nx-font-bold nx-px-1">Cloud CLI: </span>
      <span className="nx-font-bold">{release.cloudCli}</span>
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
