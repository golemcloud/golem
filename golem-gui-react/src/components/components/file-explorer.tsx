import React, { JSX, useEffect, useState } from "react";
import { ChevronRight, ChevronDown, File as FileIcon, Folder, LayoutGrid, List } from "lucide-react";
import { buildFileTree } from "./build-tree";
import useComponents from "@lib/hooks/use-component";
import { useCustomParam } from "@lib/hooks/use-custom-param";

interface FileItem {
  key: string;
  path: string;
  permissions: string;
  name?: string;
}

interface FileNode {
  name: string;
  type: 'folder' | 'file';
  children?: FileNode[];
  key?: string;
  permissions?: string;
  path?: string;
}

// Flatten tree for table view
const flattenTree = (node: FileNode, path: string = ""): FileItem[] => {
  const currentPath = `${path}/${node.name}`.replace(/^\/+/, '/');
  let files: FileItem[] = [];

  if (node.type === "file") {
    files.push({
      name: node.name,
      path: currentPath,
      permissions: node.permissions ?? "none",  // Default to "none" if no permissions
      key: node.key ?? currentPath
    });
  } else if (node.children) {
    node.children.forEach(child => {
      files = [...files, ...flattenTree(child, currentPath)];
    });
  }
  return files;
};

const TreeView: React.FC<{ fileTree: FileNode }> = ({ fileTree }) => {
  const getAllFolderPaths = (node: FileNode, path: string = ""): string[] => {
    const currentPath = `${path}/${node.name}`;
    let paths: string[] = [];

    if (node.type === "folder") {
      paths.push(currentPath);
      node.children?.forEach(child => {
        paths = [...paths, ...getAllFolderPaths(child, currentPath)];
      });
    }
    return paths;
  };

  const [expandedFolders, setExpandedFolders] = useState<Set<string>>(
    new Set(getAllFolderPaths(fileTree))
  );

  const toggleFolder = (folderPath: string) => {
    const newExpanded = new Set(expandedFolders);
    if (newExpanded.has(folderPath)) {
      newExpanded.delete(folderPath);
    } else {
      newExpanded.add(folderPath);
    }
    setExpandedFolders(newExpanded);
  };

  const renderTree = (node: FileNode, path: string = ""): JSX.Element => {
    const currentPath = `${path}/${node.name}`;
    const isExpanded = expandedFolders.has(currentPath);

    if (node.type === "folder") {
      return (
        <div key={currentPath} className="select-none">
          <div
            className="flex items-center gap-1 px-2 py-1 hover:bg-gray-200 dark:hover:bg-gray-700 rounded cursor-pointer"
            onClick={() => toggleFolder(currentPath)}
          >
            <span className="text-muted-foreground">
              {isExpanded ? <ChevronDown size={16} /> : <ChevronRight size={16} />}
            </span>
            <Folder size={16} className="text-blue-400" />
            <span className="font-medium text-foreground">{node.name}</span>
          </div>
          {isExpanded && (
            <div className="ml-4 border-l border-gray-700">
              {node.children?.map((child) => renderTree(child, currentPath))}
            </div>
          )}
        </div>
      );
    } else {
      return (
        <div key={node.key} className="flex items-center gap-1 px-2 py-1 hover:bg-gray-200 dark:hover:bg-gray-700 rounded ml-6">
          <FileIcon size={16} className="text-gray-400" />
          <span className="text-foreground">{node.name}</span>
          <span className="ml-2 text-xs px-2 py-0.5 bg-gray-700 rounded-full text-gray-300">
            {node.permissions}
          </span>
        </div>
      );
    }
  };

  return <div className="font-sans">{renderTree(fileTree)}</div>;
};

const TableView: React.FC<{ files: FileItem[] }> = ({ files }) => {
  return (
    <div className="overflow-x-auto">
      <table className="w-full">
        <thead>
          <tr className="border-b border-gray-700">
            <th className="text-left p-3 text-foreground">Name</th>
            <th className="text-left p-3 text-foreground">Path</th>
            <th className="text-left p-3 text-foreground">Permissions</th>
          </tr>
        </thead>
        <tbody>
          {files.map((file) => (
            <tr key={file.key} className="border-b border-gray-700 dark:hover:bg-gray-700 hover:bg-gray-200">
              <td className="p-3 text-muted-foreground">
                <div className="flex items-center gap-2">
                  <FileIcon size={16} className="text-gray-400" />
                  {file.name}
                </div>
              </td>
              <td className="p-3 text-muted-foreground">{file.path}</td>
              <td className="p-3 text-muted-foreground">
                <span className="text-xs px-2 py-0.5 bg-gray-700 rounded-full text-gray-300">
                  {file.permissions}
                </span>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
};

const FileExplorerCombined: React.FC = () => {
  const [viewMode, setViewMode] = useState<'tree' | 'table'>('tree');
  const [files, setFiles] = useState<FileItem[]>([]);
  const { compId } = useCustomParam();
  const { components } = useComponents(compId, "latest");
  const [latestComponent] = components;
  console.log("latestComponent", latestComponent);

  useEffect(() => {
    if (latestComponent) {
      const files = latestComponent.files;
      setFiles(files);
    }
  }, [latestComponent]);

  const fileTree = buildFileTree(files);
  const flattenedFiles = flattenTree(fileTree);

  return (
    <div className="p-6 dark:bg-[#0a0a0a] border">
      <div className="flex justify-between items-center mb-4">
        <h1 className="text-2xl font-bold dark:text-gray-100">File Explorer</h1>
        <div className="flex p-1 rounded-sm dark:bg-[#333] bg-gray-200">
          <button
            onClick={() => setViewMode('tree')}
            className={`p-2 rounded ${
              viewMode === 'tree' ? "dark:bg-black bg-gray-500 text-white hover:bg-gray-500"
                : "dark:text-gray-200 text-gray-700"
            }`}
          >
            <LayoutGrid size={20} />
          </button>
          <button
            onClick={() => setViewMode('table')}
            className={`p-2 rounded ${
              viewMode === 'table' ? "dark:bg-black bg-gray-500 text-white hover:bg-gray-500"
                : "dark:text-gray-200 text-gray-700"
            }`}
          >
            <List size={20} />
          </button>
        </div>
      </div>
      <div className="border border-gray-700 rounded-lg shadow-lg dark:bg-[#222] p-4">
        {viewMode === 'tree' ? (
          <TreeView fileTree={fileTree} />
        ) : (
          <TableView files={flattenedFiles} />
        )}
      </div>
    </div>
  );
};

export default FileExplorerCombined;
