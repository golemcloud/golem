import type { Meta, StoryObj } from "@storybook/react-vite";
import { YamlViewerModal } from "./yaml-viewer-modal";
import { fn } from "storybook/test";

const meta = {
  title: "Components/YamlViewerModal",
  component: YamlViewerModal,
  args: {
    onOpenChange: fn(),
  },
} satisfies Meta<typeof YamlViewerModal>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Open: Story = {
  args: {
    isOpen: true,
    appId: "my-shopping-app",
  },
};

export const Closed: Story = {
  args: {
    isOpen: false,
    appId: "my-shopping-app",
  },
};
