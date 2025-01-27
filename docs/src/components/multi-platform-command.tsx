import { Code, Pre, Tabs } from "nextra/components"
import { Fragment } from "react"
import { ComponentProps } from "react"

type Props = {
  commands: Command[]
  language?: string
} & Omit<ComponentProps<typeof Pre>, "children">

type Command = {
  label: string
  command: string
}

export const MultiPlatformCommand = ({ commands: command, language, ...props }: Props) => {
  const labels = command.map(c => c.label)

  // Mimicking the shiki theme
  const commands = command.map(c =>
    c.command.split("\n").map((line, i) => {
      const lines = line.split(" ")
      return (
        <span className="line" key={i}>
          {lines.map((word, i) => (
            <Fragment key={i}>
              {i === 0 ? (
                <span style={{ color: "var(--shiki-token-function)" }}>{word}</span>
              ) : (
                <span style={{ color: "var(--shiki-token-string)" }}>{word}</span>
              )}
              {i < lines.length - 1 && <span style={{ color: "var(--shiki-token-text)" }}> </span>}
            </Fragment>
          ))}
        </span>
      )
    })
  )

  return (
    <Tabs items={labels}>
      {commands.map((cmd, i) => (
        <Tabs.Tab key={i}>
          <Pre
            {...props}
            hasCopyCode={true}
            data-language={language ? language : "bash"}
            data-theme="default"
          >
            <Code data-language="bash">{cmd}</Code>
          </Pre>
        </Tabs.Tab>
      ))}
    </Tabs>
  )
}
