import "../styles/globals.css"

import type { AppProps } from "next/app"

import localFont from "next/font/local"

const font = localFont({
  src: "./Satoshi-Variable.ttf",
  display: "swap",
})

export default function MyApp({ Component, pageProps }: AppProps) {
  return (
    <div
      className={`${font.className} dark:bg-background-dark bg-background-light`}
      style={{
        // Alternate t style for Satoshi.
        fontFeatureSettings: '"ss03" 1',
      }}
    >
      <Component {...pageProps} />
    </div>
  )
}
