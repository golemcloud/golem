// Site footer columns.

export interface FooterLink {
  label: string;
  href: string;
}

export interface FooterSection {
  title: string;
  links: FooterLink[];
}

export const footerSections: FooterSection[] = [
  {
    title: "Product",
    links: [
      { label: "Use Cases", href: "/use-cases" },
      { label: "Cloud", href: "/cloud" },
    ],
  },
  {
    title: "Developers",
    links: [
      { label: "Docs", href: "https://learn.golem.cloud" },
      { label: "Downloads", href: "https://golem.cloud/developers#downloads" },
      { label: "Changelog", href: "https://github.com/golemcloud/golem/releases" },
    ],
  },
  {
    title: "Community",
    links: [
      { label: "GitHub", href: "https://github.com/golemcloud/golem" },
      { label: "Discord", href: "https://discord.com/invite/UjXeH8uG4x" },
      { label: "Blog", href: "/blog" },
    ],
  },
  {
    title: "Company",
    links: [
      { label: "About", href: "/about" },
      { label: "Careers", href: "/careers" },
      { label: "Legal", href: "/legal" },
    ],
  },
];

export const footerTagline = "The durable agent runtime. Reliability and trust by construction.";
export const footerCopyright = "© 2026 Golem Cloud, Inc. All rights reserved.";

export interface SocialLink {
  label: string;
  href: string;
  icon: "github" | "discord" | "x" | "linkedin";
}

export const footerSocials: SocialLink[] = [
  { label: "GitHub", href: "https://github.com/golemcloud/golem", icon: "github" },
  { label: "Discord", href: "https://discord.com/invite/UjXeH8uG4x", icon: "discord" },
  { label: "X", href: "https://x.com/golemcloud", icon: "x" },
  { label: "LinkedIn", href: "https://www.linkedin.com/company/golem-cloud", icon: "linkedin" },
];
