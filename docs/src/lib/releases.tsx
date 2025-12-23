export const RELEASE = {
  version: "v1.4.1",
  baseDownloadUrl: "https://github.com/golemcloud/golem/releases/download",
  baseReleaseUrl: "https://github.com/golemcloud/golem/releases/tag",

  artifacts: {
    golem: {
      "mac-arm64": "golem-aarch64-apple-darwin",
      "mac-x64": "golem-x86_64-apple-darwin",
      "linux-arm64": "golem-aarch64-unknown-linux-gnu",
      "linux-x64": "golem-x86_64-unknown-linux-gnu",
      "windows-x64": "golem-x86_64-pc-windows-msvc.exe",
    },

    "golem-cli": {
      "mac-arm64": "golem-cli-aarch64-apple-darwin",
      "mac-x64": "golem-cli-x86_64-apple-darwin",
      "linux-arm64": "golem-cli-aarch64-unknown-linux-gnu",
      "linux-x64": "golem-cli-x86_64-unknown-linux-gnu",
      "windows-x64": "golem-cli-x86_64-pc-windows-msvc.exe",
    },
  },
}

export type Artifact = keyof typeof RELEASE.artifacts
export type Platform = keyof typeof RELEASE.artifacts.golem
