import type { Meta, StoryObj } from "@storybook/react-vite";
import { ProfileManager } from "./profile-manager";

const meta = {
  title: "Components/ProfileManager",
  component: ProfileManager,
} satisfies Meta<typeof ProfileManager>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Default: Story = {};
