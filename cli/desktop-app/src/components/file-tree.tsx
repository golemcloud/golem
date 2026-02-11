import { useState } from "react";
import {
  ChevronDown,
  ChevronRight,
  File,
  Folder,
  FolderOpen,
} from "lucide-react";
import { cn } from "@/lib/utils";
import { YamlFile } from "@/types/yaml-files";

export interface FileTreeNode {
  id: string;
  name: string;
  type: "file" | "folder";
  children?: FileTreeNode[];
  data?: YamlFile;
}

interface FileTreeProps {
  nodes: FileTreeNode[];
  selectedId?: string;
  onSelect?: (node: FileTreeNode) => void;
  className?: string;
}

interface FileTreeItemProps {
  node: FileTreeNode;
  level: number;
  selectedId?: string;
  onSelect?: (node: FileTreeNode) => void;
}

const FileTreeItem = ({
  node,
  level,
  selectedId,
  onSelect,
}: FileTreeItemProps) => {
  const [isExpanded, setIsExpanded] = useState(true);
  const hasChildren = node.children && node.children.length > 0;
  const isSelected = selectedId === node.id;

  const handleToggle = () => {
    if (hasChildren) {
      setIsExpanded(!isExpanded);
    }
  };

  const handleSelect = () => {
    if (node.type === "file") {
      onSelect?.(node);
    } else {
      handleToggle();
    }
  };

  return (
    <div>
      <div
        className={cn(
          "flex items-center py-1 px-2 hover:bg-muted/50 cursor-pointer rounded-sm text-sm",
          isSelected && "bg-muted",
          level > 0 && "ml-4",
        )}
        onClick={handleSelect}
        style={{ paddingLeft: `${level * 16 + 8}px` }}
      >
        {hasChildren && (
          <button
            onClick={e => {
              e.stopPropagation();
              handleToggle();
            }}
            className="mr-1 p-0.5 hover:bg-muted rounded"
            aria-label={isExpanded ? "Collapse folder" : "Expand folder"}
          >
            {isExpanded ? (
              <ChevronDown className="h-3 w-3" />
            ) : (
              <ChevronRight className="h-3 w-3" />
            )}
          </button>
        )}

        {!hasChildren && (
          <div className="w-4 mr-1" /> // Spacer for alignment
        )}

        {node.type === "folder" ? (
          isExpanded ? (
            <FolderOpen className="h-4 w-4 mr-2 text-blue-500" />
          ) : (
            <Folder className="h-4 w-4 mr-2 text-blue-500" />
          )
        ) : (
          <File className="h-4 w-4 mr-2 text-gray-500" />
        )}

        <span className="truncate">{node.name}</span>
      </div>

      {hasChildren && isExpanded && (
        <div>
          {node.children!.map(child => (
            <FileTreeItem
              key={child.id}
              node={child}
              level={level + 1}
              selectedId={selectedId}
              onSelect={onSelect}
            />
          ))}
        </div>
      )}
    </div>
  );
};

export const FileTree = ({
  nodes,
  selectedId,
  onSelect,
  className,
}: FileTreeProps) => {
  return (
    <div className={cn("w-full", className)}>
      {nodes.map(node => (
        <FileTreeItem
          key={node.id}
          node={node}
          level={0}
          selectedId={selectedId}
          onSelect={onSelect}
        />
      ))}
    </div>
  );
};
