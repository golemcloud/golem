"use client"

import { useEffect } from "react"

export function PlatformDetector() {
  useEffect(() => {
    if (typeof window !== "undefined") {
      const platform = navigator.platform.toLowerCase()
      const userAgent = navigator.userAgent.toLowerCase()
      const tabButtons = document.querySelectorAll('[role="tab"]')
      if (tabButtons.length > 0) {
        let osTabIndex = 0

        if (platform.includes("mac") || userAgent.includes("mac") || userAgent.includes("darwin")) {
          osTabIndex = 1
        } else if (platform.includes("linux") || platform.includes("x11")) {
          osTabIndex = 2
        }

        if (tabButtons[osTabIndex]) {
          setTimeout(() => {
            ;(tabButtons[osTabIndex] as HTMLElement).click()

            if (osTabIndex === 1) {
              setTimeout(() => {
                const macTabs = document.querySelectorAll(
                  '[role="tabpanel"]:not([hidden]) [role="tab"]'
                )
                if (macTabs.length > 0) {
                  const isAppleSilicon =
                    /arm|aarch64/i.test(navigator.platform) ||
                    (userAgent.includes("mac") &&
                      (/macbookpro1[78]|macbookair1[01]|macmini9|imac2[34]/i.test(userAgent) ||
                        (typeof window.navigator.hardwareConcurrency !== "undefined" &&
                          window.navigator.hardwareConcurrency >= 8)))

                  const archIndex = isAppleSilicon ? 1 : 0

                  if (macTabs[archIndex]) {
                    ;(macTabs[archIndex] as HTMLElement).click()
                  }
                }
              }, 300)
            }
          }, 300)
        }
      }
    }
  }, [])

  return null
}
