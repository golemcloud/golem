import type { Meta, StoryObj } from "@storybook/react-vite";
import Modal from "./Modal";
import { fn, userEvent, within, expect } from "storybook/test";

const meta = {
  title: "Components/Modal",
  component: Modal,
  argTypes: {
    isOpen: { control: "boolean" },
    onClose: { action: "onClose" },
  },
  args: {
    onClose: fn(),
  },
} satisfies Meta<typeof Modal>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Open: Story = {
  args: {
    isOpen: true,
    children: (
      <div className="p-6">
        <h2 className="text-lg font-bold mb-4">Sample Modal</h2>
        <p className="text-muted-foreground">
          This is a sample modal with some content to demonstrate the component.
        </p>
      </div>
    ),
  },
  play: async ({ canvasElement, args }) => {
    const canvas = within(canvasElement);
    await expect(canvas.getByText("Sample Modal")).toBeInTheDocument();

    const closeButton = canvas.getByRole("button");
    await userEvent.click(closeButton);
    await expect(args.onClose).toHaveBeenCalled();
  },
};

export const Closed: Story = {
  args: {
    isOpen: false,
    children: <div>This should not be visible</div>,
  },
};

export const WithFormContent: Story = {
  args: {
    isOpen: true,
    children: (
      <div className="p-6">
        <h2 className="text-lg font-bold mb-4">Edit Profile</h2>
        <form className="space-y-4">
          <div>
            <label
              htmlFor="profile-name"
              className="block text-sm font-medium mb-1"
            >
              Name
            </label>
            <input
              id="profile-name"
              type="text"
              className="w-full border border-border rounded px-3 py-2 bg-background text-foreground"
              defaultValue="my-profile"
            />
          </div>
          <div>
            <label
              htmlFor="profile-url"
              className="block text-sm font-medium mb-1"
            >
              URL
            </label>
            <input
              id="profile-url"
              type="text"
              className="w-full border border-border rounded px-3 py-2 bg-background text-foreground"
              defaultValue="http://localhost:9881"
            />
          </div>
          <div className="flex justify-end gap-2">
            <button className="px-4 py-2 border border-border rounded bg-background text-foreground hover:bg-muted">
              Cancel
            </button>
            <button className="px-4 py-2 bg-primary text-primary-foreground rounded hover:bg-primary/90">
              Save
            </button>
          </div>
        </form>
      </div>
    ),
  },
  play: async ({ canvasElement, args }) => {
    const canvas = within(canvasElement);

    const nameInput = canvas.getByDisplayValue("my-profile");
    await expect(nameInput).toBeInTheDocument();

    const urlInput = canvas.getByDisplayValue("http://localhost:9881");
    await expect(urlInput).toBeInTheDocument();

    await userEvent.clear(nameInput);
    await userEvent.type(nameInput, "new-profile");

    // Click the X close button (first button in the modal, with the X icon)
    const buttons = canvas.getAllByRole("button");
    // The X close button is the first one rendered by the Modal component
    const closeButton = buttons[0]!;
    await userEvent.click(closeButton);
    await expect(args.onClose).toHaveBeenCalled();
  },
};
