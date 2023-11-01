import {
  DiscordLogoIcon,
  EnvelopeClosedIcon,
  GitHubLogoIcon,
  TwitterLogoIcon,
} from "@radix-ui/react-icons"
import { GolemLogo } from "./golem-logo"

export function Footer() {
  const year = new Date().getFullYear()
  return (
    <footer className="mt-auto nx-bg-gray-100 nx-pb-[env(safe-area-inset-bottom)] dark:nx-bg-neutral-900 print:nx-bg-transparent">
      <div
        className={
          "overflow-hidden py-10 nx-border-t dark:nx-border-neutral-800 contrast-more:nx-border-neutral-400 dark:contrast-more:nx-border-neutral-400"
        }
      >
        <div className="mx-auto max-w-7xl px-6 pb-8 pt-12 lg:px-8">
          <div className="lg:grid lg:grid-cols-3 lg:gap-8">
            <div className="flex flex-col items-start justify-center gap-5">
              <GolemLogo />

              <div className="flex justify-center gap-5">
                {socials.map(item => (
                  <a
                    key={item.href}
                    href={item.href}
                    target="_blank"
                    rel="noopener"
                    className="nx-h-7 nx-rounded-md nx-transition-colors nx-text-gray-600 dark:nx-text-gray-400 nx-px-2 hover:nx-bg-gray-100 hover:nx-text-gray-900 dark:hover:nx-bg-primary-100/5 dark:hover:nx-text-gray-50 border grid place-items-center dark:nx-border-neutral-700 contrast-more:nx-border-neutral-400 dark:contrast-more:nx-border-neutral-400"
                  >
                    <span className="sr-only">{item.name}</span>
                    <item.icon className="h-5 w-5" aria-hidden="true" />
                  </a>
                ))}
              </div>

              <div className="text-center text-sm leading-5">
                <div>© {year} Ziverge Inc.</div>
              </div>
            </div>

            <div className="mt-16 grid grid-cols-2 gap-8 lg:col-span-2 lg:mt-0">
              <nav>
                <h3 className="text-sm font-semibold leading-6">Golem</h3>
                <ul role="list" className="mt-6 space-y-4">
                  {golem.map(item => (
                    <li key={item.name}>
                      <a
                        href={item.href}
                        target="_blank"
                        rel="noopener"
                        className="text-sm leading-6 nx-inline-block nx-text-gray-500 hover:nx-text-gray-900 dark:nx-text-gray-400 dark:hover:nx-text-gray-300 contrast-more:nx-text-gray-900 contrast-more:nx-underline contrast-more:dark:nx-text-gray-50 nx-w-full nx-break-words"
                      >
                        {item.name}
                      </a>
                    </li>
                  ))}
                </ul>
              </nav>

              <nav>
                <h3 className="text-sm font-semibold leading-6">Support</h3>
                <ul role="list" className="mt-6 space-y-4">
                  {support.map(item => (
                    <li key={item.name}>
                      <a
                        href={item.href}
                        target="_blank"
                        rel="noopener"
                        className="text-sm leading-6 nx-inline-block nx-text-gray-500 hover:nx-text-gray-900 dark:nx-text-gray-400 dark:hover:nx-text-gray-300 contrast-more:nx-text-gray-900 contrast-more:nx-underline contrast-more:dark:nx-text-gray-50 nx-w-full nx-break-words"
                      >
                        {item.name}
                      </a>
                    </li>
                  ))}
                </ul>
              </nav>
            </div>
          </div>
        </div>
      </div>
    </footer>
  )
}

const support = [
  { name: "Blog", href: "https://www.golem.cloud/blog" },
  { name: "Help Center", href: "https://support.golem.cloud" },
]

const golem = [
  { name: "About", href: "https://www.golem.cloud" },
  { name: "Console", href: "https://console.golem.cloud" },
]

const socials = [
  {
    name: "Github",
    href: "https://github.com/golemcloud",
    icon: GitHubLogoIcon,
  },
  {
    name: "Twitter",
    href: "https://twitter.com/golemcloud",
    icon: TwitterLogoIcon,
  },
  {
    name: "Email",
    href: "mailto:contact@golem.cloud",
    icon: EnvelopeClosedIcon,
  },
  {
    name: "Discord",
    href: "https://discord.gg/UjXeH8uG4x",
    icon: DiscordLogoIcon,
  },
] as const
