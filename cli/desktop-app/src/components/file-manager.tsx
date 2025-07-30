"use client";

import type React from "react";
import { useState } from "react";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/components/ui/collapsible";
import { ChevronDown, ChevronRight, File, Folder } from "lucide-react";
import { cn, parseFileStructure } from "@/lib/utils";
import { FileStructure } from "@/types/component.ts";

export interface FileNode {
  name: string;
  type: "file" | "folder";
  children?: FileNode[];
  permissions?: string;
}

interface FolderStructureProps {
  data: FileStructure[];
}

const FolderStructureNode: React.FC<{ node: FileNode; level: number }> = ({
  node,
  level,
}) => {
  const [isOpen, setIsOpen] = useState(true);

  const indent = level * 16;

  if (node.type === "file") {
    return (
      <div
        className="flex items-center py-2 px-2 rounded-md hover:bg-gray-100 dark:hover:bg-gray-400 transition-colors duration-150"
        style={{ paddingLeft: `${indent + 8}px` }}
      >
        <File className="w-4 h-4 mr-2 text-blue-500 dark:text-blue-400" />
        <span className="text-sm">{node.name}</span>
        {node.permissions && (
          <span className="ml-2 text-xs px-2 py-1 rounded-full bg-gray-200 dark:bg-gray-600 text-gray-700 dark:text-gray-300">
            {node.permissions}
          </span>
        )}
      </div>
    );
  }

  return (
    <Collapsible open={isOpen} onOpenChange={setIsOpen}>
      <CollapsibleTrigger
        className="flex items-center py-2 px-2 w-full text-left rounded-md hover:bg-gray-100 dark:hover:bg-gray-400 transition-colors duration-150"
        style={{ paddingLeft: `${indent}px` }}
      >
        {isOpen ? (
          <ChevronDown className="w-4 h-4 mr-2 text-gray-500 dark:text-gray-400" />
        ) : (
          <ChevronRight className="w-4 h-4 mr-2 text-gray-500 dark:text-gray-400" />
        )}
        <Folder
          className={cn(
            "w-4 h-4 mr-2",
            isOpen
              ? "text-yellow-500 dark:text-yellow-400"
              : "text-gray-400 dark:text-gray-500",
          )}
        />
        <span className="text-sm font-medium">{node.name}</span>
      </CollapsibleTrigger>
      <CollapsibleContent>
        {node.children?.map((child, index) => (
          <FolderStructureNode key={index} node={child} level={level + 1} />
        ))}
      </CollapsibleContent>
    </Collapsible>
  );
};

export const FolderStructure: React.FC<FolderStructureProps> = ({ data }) => {
  const rootNode = parseFileStructure(data);

  return (
    <div className="space-y-4">
      <div className="border rounded-lg p-4 shadow-md transition-all duration-300 ease-in-out">
        {data.length > 0 ? (
          <FolderStructureNode node={rootNode} level={0} />
        ) : (
          <div className="flex items-center justify-center text-center">
            No files found
          </div>
        )}
      </div>
    </div>
  );
};
