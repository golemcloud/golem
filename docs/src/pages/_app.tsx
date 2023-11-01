import "../styles/globals.css"

import type { AppProps } from "next/app"

import localFont from "next/font/local"

const sansFont = localFont({
  src: "./Satoshi.ttf",
  display: "swap",
  variable: "--satoshi",
})

export default function MyApp({ Component, pageProps }: AppProps) {
  return (
    <div
      className={`${sansFont.variable} dark:bg-background-dark bg-background-light font-sans`}
      style={{
        // Alternate t style for Satoshi.
        fontFeatureSettings: '"ss03" 1',
      }}
    >
      <Component {...pageProps} />
    </div>
  )
}
