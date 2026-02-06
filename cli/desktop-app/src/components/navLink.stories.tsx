import type { Meta, StoryObj } from "@storybook/react-vite";
import NavLink from "./navLink";
import { within, expect, userEvent } from "storybook/test";

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
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const link = canvas.getByText("Dashboard");
    await expect(link).toBeInTheDocument();
    await expect(link.className).toContain("border-primary-soft");
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
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const link = canvas.getByText("Settings");
    await expect(link).toBeInTheDocument();
    await expect(link.className).toContain("text-gray-500");
    await userEvent.click(link);
  },
};
