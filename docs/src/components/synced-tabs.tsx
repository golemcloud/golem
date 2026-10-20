"use client"

import { Tabs } from "nextra/components"
import { type ComponentProps, useEffect, useRef, useState } from "react"

const CHANGE_EVENT = "golem-tabs-change"

type ChangeDetail = { key: string; index: number; source: Element }

// Like Nextra's Tabs with a storageKey, except that choosing a tab updates only this
// group and the groups after it on the page, so content above the reader never resizes.
export function SyncedTabs({
  storageKey,
  defaultIndex = 0,
  onChange,
  ...props
}: ComponentProps<typeof Tabs>) {
  const ref = useRef<HTMLDivElement>(null)
  const [selectedIndex, setSelectedIndex] = useState(defaultIndex)

  useEffect(() => {
    if (!storageKey) return
    const stored = localStorage.getItem(storageKey)
    // The stored choice can only be read after hydration, or the markup would differ from the server's.
    // eslint-disable-next-line react-hooks/set-state-in-effect
    if (stored !== null && !Number.isNaN(Number(stored))) setSelectedIndex(Number(stored))

    const onLocalChange = (event: Event) => {
      const { key, index, source } = (event as CustomEvent<ChangeDetail>).detail
      const self = ref.current
      if (key !== storageKey || !self || source === self) return
      if (source.compareDocumentPosition(self) & Node.DOCUMENT_POSITION_FOLLOWING) {
        setSelectedIndex(index)
      }
    }
    const onOtherWindowChange = (event: StorageEvent) => {
      if (event.key === storageKey && event.newValue !== null) {
        setSelectedIndex(Number(event.newValue))
      }
    }
    window.addEventListener(CHANGE_EVENT, onLocalChange)
    window.addEventListener("storage", onOtherWindowChange)
    return () => {
      window.removeEventListener(CHANGE_EVENT, onLocalChange)
      window.removeEventListener("storage", onOtherWindowChange)
    }
  }, [storageKey])

  const handleChange = (index: number) => {
    setSelectedIndex(index)
    onChange?.(index)
    if (!storageKey || !ref.current) return
    localStorage.setItem(storageKey, String(index))
    window.dispatchEvent(
      new CustomEvent<ChangeDetail>(CHANGE_EVENT, {
        detail: { key: storageKey, index, source: ref.current },
      })
    )
  }

  return (
    <div ref={ref}>
      <Tabs {...props} selectedIndex={selectedIndex} onChange={handleChange} />
    </div>
  )
}
