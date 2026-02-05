import type { Meta, StoryObj } from "@storybook/react-vite";
import NavLink from "./navLink";

const meta = {
  title: "Components/NavLink",
  component: NavLink,
} satisfies Meta<typeof NavLink>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Active: Story = {
  args: {
    to: "/",
    children: "Dashboard",
  },
  parameters: {
    router: { route: "/" },
  },
};

export const Inactive: Story = {
  args: {
    to: "/settings",
    children: "Settings",
  },
  parameters: {
    router: { route: "/dashboard" },
  },
};
