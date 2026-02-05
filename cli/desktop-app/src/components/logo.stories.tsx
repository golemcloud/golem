import type { Meta, StoryObj } from "@storybook/react-vite";
import { Logo } from "./logo";

const meta = {
  title: "Components/Logo",
  component: Logo,
} satisfies Meta<typeof Logo>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Default: Story = {};
