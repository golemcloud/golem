import type { Meta, StoryObj } from "@storybook/react-vite";
import { ModeToggle } from "./mode-toggle";

const meta = {
  title: "Components/ModeToggle",
  component: ModeToggle,
} satisfies Meta<typeof ModeToggle>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Default: Story = {};
