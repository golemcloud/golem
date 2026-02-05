import type { Meta, StoryObj } from "@storybook/react-vite";
import { FileTree, FileTreeNode } from "./file-tree";
import { fn } from "storybook/test";

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
};

export const SimpleFiles: Story = {
  args: {
    nodes: [
      { id: "1", name: "index.ts", type: "file" },
      { id: "2", name: "utils.ts", type: "file" },
      { id: "3", name: "README.md", type: "file" },
    ],
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
