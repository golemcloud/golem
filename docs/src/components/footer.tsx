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
    <footer className="mt-auto bg-gray-100 pb-[env(safe-area-inset-bottom)] dark:bg-neutral-900 print:bg-transparent">
      <div
        className={
          "overflow-hidden border-t py-10 contrast-more:border-neutral-400 dark:border-neutral-800 dark:contrast-more:border-neutral-400"
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
                    rel="noopener noreferrer"
                    className="grid h-7 place-items-center rounded-md border px-2 text-gray-600 transition-colors hover:bg-gray-100 hover:text-gray-900 contrast-more:border-neutral-400 dark:border-neutral-700 dark:text-gray-400 dark:hover:bg-[color-mix(in_srgb,var(--x-color-primary-100)_5%,transparent)] dark:hover:text-gray-50 dark:contrast-more:border-neutral-400"
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
                <ul className="mt-6 space-y-4">
                  {golem.map(item => (
                    <li key={item.name}>
                      <a
                        href={item.href}
                        target="_blank"
                        rel="noopener noreferrer"
                        className="inline-block w-full break-words text-sm leading-6 text-gray-500 hover:text-gray-900 contrast-more:text-gray-900 contrast-more:underline dark:text-gray-400 dark:hover:text-gray-300 contrast-more:dark:text-gray-50"
                      >
                        {item.name}
                      </a>
                    </li>
                  ))}
                </ul>
              </nav>

              <nav>
                <h3 className="text-sm font-semibold leading-6">Support</h3>
                <ul className="mt-6 space-y-4">
                  {support.map(item => (
                    <li key={item.name}>
                      <a
                        href={item.href}
                        target="_blank"
                        rel="noopener noreferrer"
                        className="inline-block w-full break-words text-sm leading-6 text-gray-500 hover:text-gray-900 contrast-more:text-gray-900 contrast-more:underline dark:text-gray-400 dark:hover:text-gray-300 contrast-more:dark:text-gray-50"
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
  { name: "Help Center", href: "https://help.golem.cloud" },
]

const golem = [{ name: "About", href: "https://www.golem.cloud" }]

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
