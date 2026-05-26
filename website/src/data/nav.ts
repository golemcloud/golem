// Top-nav link list.

export interface NavItem {
  label: string;
  href: string;
}

export const navItems: NavItem[] = [
  { label: "Use Cases", href: "/use-cases" },
  { label: "Cloud", href: "/cloud" },
  { label: "Docs", href: "https://learn.golem.cloud" },
  { label: "Blog", href: "/blog" },
];
