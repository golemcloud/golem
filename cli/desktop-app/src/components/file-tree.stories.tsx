import type { Meta, StoryObj } from "@storybook/react-vite";
import { FileTree, FileTreeNode } from "./file-tree";
import { fn, userEvent, within, expect } from "storybook/test";

const meta = {
  title: "Components/FileTree",
  component: FileTree,
  args: {
    onSelect: fn(),
  },
} satisfies Meta<typeof FileTree>;

export default meta;
type Story = StoryObj<typeof meta>;

const nestedNodes: FileTreeNode[] = [
  {
    id: "root",
    name: "golem.yaml",
    type: "file",
  },
  {
    id: "common",
    name: "common-1",
    type: "folder",
    children: [
      {
        id: "common-1-yaml",
        name: "golem.yaml",
        type: "file",
      },
    ],
  },
  {
    id: "components",
    name: "components-default",
    type: "folder",
    children: [
      {
        id: "comp-shopping",
        name: "shopping-cart",
        type: "folder",
        children: [
          { id: "comp-shopping-yaml", name: "golem.yaml", type: "file" },
          { id: "comp-shopping-wit", name: "main.wit", type: "file" },
        ],
      },
      {
        id: "comp-auth",
        name: "auth-service",
        type: "folder",
        children: [
          { id: "comp-auth-yaml", name: "golem.yaml", type: "file" },
        ],
      },
    ],
  },
];

export const NestedFolders: Story = {
  args: {
    nodes: nestedNodes,
  },
  play: async ({ canvasElement, args }) => {
    const canvas = within(canvasElement);

    // Verify top-level items are visible
    await expect(canvas.getByText("common-1")).toBeInTheDocument();
    await expect(canvas.getByText("components-default")).toBeInTheDocument();

    // Click a file -> assert onSelect called with node data
    const rootFile = canvas.getAllByText("golem.yaml")[0]!;
    await userEvent.click(rootFile);
    await expect(args.onSelect).toHaveBeenCalledWith(
      expect.objectContaining({ id: "root", name: "golem.yaml", type: "file" }),
    );

    // Collapse a folder by clicking its chevron button
    const folderButtons = canvasElement.querySelectorAll("button");
    // The first chevron button belongs to "common-1" folder
    const commonChevron = folderButtons[0]!;
    await userEvent.click(commonChevron);

    // After collapsing, the child "golem.yaml" inside common-1 should be hidden
    // (but the root golem.yaml and components golem.yaml are still visible)
    // Re-expand
    await userEvent.click(commonChevron);
  },
};

export const SimpleFiles: Story = {
  args: {
    nodes: [
      { id: "1", name: "index.ts", type: "file" },
      { id: "2", name: "utils.ts", type: "file" },
      { id: "3", name: "README.md", type: "file" },
    ],
  },
  play: async ({ canvasElement, args }) => {
    const canvas = within(canvasElement);

    const indexFile = canvas.getByText("index.ts");
    await userEvent.click(indexFile);
    await expect(args.onSelect).toHaveBeenCalledWith(
      expect.objectContaining({ id: "1", name: "index.ts", type: "file" }),
    );
  },
};

export const WithSelection: Story = {
  args: {
    nodes: nestedNodes,
    selectedId: "comp-shopping-yaml",
  },
};

export const Empty: Story = {
  args: {
    nodes: [],
  },
};
