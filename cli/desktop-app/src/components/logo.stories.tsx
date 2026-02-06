import type { Meta, StoryObj } from "@storybook/react-vite";
import { Logo } from "./logo";
import { expect } from "storybook/test";

const meta = {
  title: "Components/Logo",
  component: Logo,
} satisfies Meta<typeof Logo>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Default: Story = {
  play: async ({ canvasElement }) => {
    const svg = canvasElement.querySelector("svg.logo-light");
    await expect(svg).toBeInTheDocument();
  },
};
