import type { Meta, StoryObj } from "@storybook/react-vite";
import Modal from "./Modal";

const meta = {
  title: "Components/Modal",
  component: Modal,
  argTypes: {
    isOpen: { control: "boolean" },
    onClose: { action: "onClose" },
  },
} satisfies Meta<typeof Modal>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Open: Story = {
  args: {
    isOpen: true,
    onClose: () => {},
    children: (
      <div className="p-6">
        <h2 className="text-lg font-bold mb-4">Sample Modal</h2>
        <p className="text-gray-600">
          This is a sample modal with some content to demonstrate the component.
        </p>
      </div>
    ),
  },
};

export const Closed: Story = {
  args: {
    isOpen: false,
    onClose: () => {},
    children: <div>This should not be visible</div>,
  },
};

export const WithFormContent: Story = {
  args: {
    isOpen: true,
    onClose: () => {},
    children: (
      <div className="p-6">
        <h2 className="text-lg font-bold mb-4">Edit Profile</h2>
        <form className="space-y-4">
          <div>
            <label className="block text-sm font-medium mb-1">Name</label>
            <input
              type="text"
              className="w-full border rounded px-3 py-2"
              defaultValue="my-profile"
            />
          </div>
          <div>
            <label className="block text-sm font-medium mb-1">URL</label>
            <input
              type="text"
              className="w-full border rounded px-3 py-2"
              defaultValue="http://localhost:9881"
            />
          </div>
          <div className="flex justify-end gap-2">
            <button className="px-4 py-2 border rounded">Cancel</button>
            <button className="px-4 py-2 bg-blue-500 text-white rounded">
              Save
            </button>
          </div>
        </form>
      </div>
    ),
  },
};
