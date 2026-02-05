import type { Meta, StoryObj } from "@storybook/react-vite";
import { CreateProfileDialog } from "./create-profile-dialog";
import { fn } from "storybook/test";

const meta = {
  title: "Components/CreateProfileDialog",
  component: CreateProfileDialog,
  args: {
    onProfileCreated: fn(),
  },
} satisfies Meta<typeof CreateProfileDialog>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Default: Story = {};
