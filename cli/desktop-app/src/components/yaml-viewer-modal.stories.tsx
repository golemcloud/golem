import type { Meta, StoryObj } from "@storybook/react-vite";
import { YamlViewerModal } from "./yaml-viewer-modal";
import { fn, expect, screen } from "storybook/test";

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
  play: async () => {
    // Dialog renders in a portal, use screen.
    // The component fetches YAML files from the API on mount.
    // In Storybook the API is not available, so it shows the loading/empty state dialog.
    // Verify the dialog container is rendered.
    const dialog = await screen.findByRole("dialog");
    await expect(dialog).toBeInTheDocument();
  },
};

export const Closed: Story = {
  args: {
    isOpen: false,
    appId: "my-shopping-app",
  },
  play: async () => {
    // Verify no dialog in DOM
    const dialog = screen.queryByRole("dialog");
    await expect(dialog).not.toBeInTheDocument();
  },
};
