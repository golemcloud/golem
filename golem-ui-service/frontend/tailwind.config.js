/** @type {import('tailwindcss').Config} */
export default {
  content: ["./index.html", "./src/**/*.{js,ts,jsx,tsx}"],
  theme: {
    extend: {
      colors: {
        background: "hsl(var(--golem-background))",
        foreground: "hsl(var(--golem-foreground))",
        border: "hsl(var(--golem-border))",
        input: "hsl(var(--golem-input))",
        ring: "hsl(var(--golem-ring))",
        primary: {
          DEFAULT: "hsl(var(--golem-primary))",
          foreground: "hsl(var(--golem-primary-foreground))",
          background: "hsl(var(--golem-primary-background))",
          soft: "hsl(var(--golem-primary-soft))",
          accent: "hsl(var(--golem-primary-accent))",
          border: "hsl(var(--golem-primary-border))",
        },
        destructive: {
          DEFAULT: "hsl(var(--golem-destructive))",
          foreground: "hsl(var(--golem-destructive-foreground))",
          background: "hsl(var(--golem-destructive-background))",
          soft: "hsl(var(--golem-destructive-soft))",
          accent: "hsl(var(--golem-destructive-accent))",
          border: "hsl(var(--golem-destructive-border))",
        },
        success: {
          DEFAULT: "hsl(var(--golem-success))",
          foreground: "hsl(var(--golem-success-foreground))",
          background: "hsl(var(--golem-success-background))",
          soft: "hsl(var(--golem-success-soft))",
          accent: "hsl(var(--golem-success-accent))",
          border: "hsl(var(--golem-success-border))",
        },
        sidebar: {
          background: "hsl(var(--golem-sidebar-background))",
          foreground: "hsl(var(--golem-sidebar-foreground))",
          primary: "hsl(var(--golem-sidebar-primary))",
          accent: "hsl(var(--golem-sidebar-accent))",
          border: "hsl(var(--golem-sidebar-border))",
        },
        card: {
          DEFAULT: "hsl(var(--golem-card))",
          foreground: "hsl(var(--golem-card-foreground))",
        },
        popover: {
          DEFAULT: "hsl(var(--golem-popover))",
          foreground: "hsl(var(--golem-popover-foreground))",
        },
        muted: {
          DEFAULT: "hsl(var(--golem-muted))",
          foreground: "hsl(var(--golem-muted-foreground))",
        },
        accent: {
          DEFAULT: "hsl(var(--golem-accent))",
          foreground: "hsl(var(--golem-accent-foreground))",
        },
      },
      borderRadius: {
        DEFAULT: "var(--golem-radius)",
      },
    },
  },
  plugins: [],
};
