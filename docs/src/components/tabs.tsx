import { Tabs as NextraTabs } from "nextra/components"
import type { ComponentProps } from "react"
import { SyncedTabs } from "./synced-tabs"

export const Tabs = Object.assign(
  (props: ComponentProps<typeof NextraTabs>) => <SyncedTabs {...props} />,
  { Tab: NextraTabs.Tab }
)
