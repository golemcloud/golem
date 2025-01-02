import React, { useState, useCallback } from "react";
import { Folder, File, ChevronDown, ChevronRight, Trash } from "lucide-react";
import { useDropzone } from "react-dropzone";
import { useDrag, useDrop, DndProvider } from "react-dnd";
import { HTML5Backend } from "react-dnd-html5-backend";
import ErrorBoundary from "@/components/errorBoundary";

interface FileItem {
  id: string;
  name: string;
  type: "file" | "folder";
  size?: number;
  children?: FileItem[];
}

const DraggableFileItem: React.FC<{
  item: FileItem;
  moveItem: (draggedId: string, targetId: string) => void;
  toggleFolder: (folderId: string) => void;
  expandedFolders: Set<string>;
  handleDelete: (id: string) => void;
  formatFileSize: (bytes?: number) => string;
  depth: number;
  renameFolder: (id: string, newName: string) => void;
}> = ({
  item,
  moveItem,
  toggleFolder,
  expandedFolders,
  handleDelete,
  formatFileSize,
  depth,
  renameFolder,
}) => {
  const [{ isDragging }, dragRef] = useDrag(() => ({
    type: "fileItem",
    item: { id: item.id },
    collect: (monitor) => ({
      isDragging: monitor.isDragging(),
    }),
  }));

  const [, dropRef] = useDrop(() => ({
    accept: "fileItem",
    drop: (draggedItem: { id: string }) => {
      if (draggedItem.id !== item.id) {
        moveItem(draggedItem.id, item.id);
      }
    },
  }));

  const [isEditing, setIsEditing] = useState(false);
  const [newName, setNewName] = useState(item.name);

  const handleRename = () => {
    if (newName.trim()) {
      renameFolder(item.id, newName);
      setIsEditing(false);
    }
  };

  return (
    <ErrorBoundary>
      <div
        ref={dropRef}
        className={`flex items-center px-${depth * 2} py-2 hover:bg-gray-50 ${
          isDragging ? "opacity-50" : "opacity-100"
        }`}
      >
        <span className={`w-${depth * 6}`} />
        {item.type === "folder" && (
          <button
            onClick={() => toggleFolder(item.id)}
            className="mr-2 text-gray-400"
          >
            {expandedFolders.has(item.id) ? (
              <ChevronDown className="h-4 w-4" />
            ) : (
              <ChevronRight className="h-4 w-4" />
            )}
          </button>
        )}

        <div ref={dragRef} className="flex items-center w-full">
          {item.type === "folder" ? (
            <Folder className="h-5 w-5 text-gray-400 mr-2" />
          ) : (
            <File className="h-5 w-5 text-gray-400 mr-2" />
          )}
          {isEditing ? (
            <input
              type="text"
              value={newName}
              onChange={(e) => setNewName(e.target.value)}
              onBlur={handleRename}
              onKeyDown={(e) => {
                if (e.key === "Enter") handleRename();
              }}
              className="flex-1 p-1 border border-gray-300 rounded"
            />
          ) : (
            <span
              className="flex-1 cursor-pointer"
              onClick={() => setIsEditing(true)}
            >
              {item.name}
            </span>
          )}
          {item.size && (
            <span className="text-sm text-gray-500 mr-4">
              {formatFileSize(item.size)}
            </span>
          )}
          <button
            className="p-1 text-gray-400 hover:text-gray-600"
            onClick={() => handleDelete(item.id)}
          >
            <Trash className="h-4 w-4" />
          </button>
        </div>
      </div>
    </ErrorBoundary>
  );
};

const FileManager = () => {
  const [files, setFiles] = useState<FileItem[]>([]);
  const [expandedFolders, setExpandedFolders] = useState<Set<string>>(
    new Set()
  );

  const onDrop = useCallback((acceptedFiles: File[]) => {
    const newFiles = acceptedFiles.map((file) => ({
      id: Math.random().toString(36).substr(2, 9),
      name: file.name,
      type: "file" as const,
      size: file.size,
    }));
    setFiles((prev) => [...prev, ...newFiles]);
  }, []);

  const { getRootProps, getInputProps, isDragActive } = useDropzone({ onDrop });

  const toggleFolder = (folderId: string) => {
    setExpandedFolders((prev) => {
      const next = new Set(prev);
      if (next.has(folderId)) {
        next.delete(folderId);
      } else {
        next.add(folderId);
      }
      return next;
    });
  };

  const createNewFolder = () => {
    const newFolder: FileItem = {
      id: Math.random().toString(36).substr(2, 9),
      name: "New Folder",
      type: "folder",
      children: [],
    };
    setFiles((prev) => [...prev, newFolder]);
  };

  const formatFileSize = (bytes?: number) => {
    if (!bytes) return "";
    const kb = bytes / 1024;
    return `${kb.toFixed(0)} KB`;
  };

  const handleDelete = (id: string) => {
    setFiles((prev) => prev.filter((item) => item.id !== id));
  };

  const moveItem = (draggedId: string, targetId: string) => {
    const findAndRemoveItem = (
      items: FileItem[],
      id: string
    ): [FileItem | null, FileItem[]] => {
      let removedItem: FileItem | null = null;
      const updatedItems = items
        .map((item) => {
          if (item.id === id) {
            removedItem = item;
            return null;
          }
          if (item.children) {
            const [childItem, updatedChildren] = findAndRemoveItem(
              item.children,
              id
            );
            if (childItem) removedItem = childItem;
            return { ...item, children: updatedChildren };
          }
          return item;
        })
        .filter(Boolean) as FileItem[];
      return [removedItem, updatedItems];
    };

    const insertIntoFolder = (
      items: FileItem[],
      targetId: string,
      itemToInsert: FileItem
    ): FileItem[] => {
      return items.map((item) => {
        if (item.id === targetId && item.type === "folder") {
          return {
            ...item,
            children: [...(item.children || []), itemToInsert],
          };
        }
        if (item.children) {
          return {
            ...item,
            children: insertIntoFolder(item.children, targetId, itemToInsert),
          };
        }
        return item;
      });
    };

    setFiles((prev) => {
      const [draggedItem, remainingFiles] = findAndRemoveItem(prev, draggedId);
      if (!draggedItem) return prev;
      return insertIntoFolder(remainingFiles, targetId, draggedItem);
    });
  };

  const renameFolder = (id: string, newName: string) => {
    setFiles((prev) => {
      const updateItemName = (items: FileItem[]): FileItem[] => {
        return items.map((item) => {
          if (item.id === id) {
            return { ...item, name: newName };
          }
          if (item.children) {
            return { ...item, children: updateItemName(item.children) };
          }
          return item;
        });
      };
      return updateItemName(prev);
    });
  };

  const renderTree = (items: FileItem[], depth = 0) => {
    return items.map((item) => (
      <div key={item.id}>
        <DraggableFileItem
          item={item}
          moveItem={moveItem}
          toggleFolder={toggleFolder}
          expandedFolders={expandedFolders}
          handleDelete={handleDelete}
          formatFileSize={formatFileSize}
          renameFolder={renameFolder}
          depth={depth}
        />
        {item.type === "folder" &&
          expandedFolders.has(item.id) &&
          item.children &&
          renderTree(item.children, depth + 1)}
      </div>
    ));
  };

  return (
    <ErrorBoundary>
      <DndProvider backend={HTML5Backend}>
        <div>
          <label className="block text-sm font-medium text-gray-700 mb-1">
            Initial Files
          </label>

          <p className="text-sm text-gray-600 mb-3">
            Files available to your workers at runtime.
          </p>

          <div
            {...getRootProps()}
            className="border-2 border-dashed border-gray-200 rounded-lg p-8 hover:border-gray-400"
          >
            <input {...getInputProps()} />

            <div className="flex flex-col items-center justify-center text-center">
              <p className="text-sm text-gray-600">
                {isDragActive ? "Drop files here" : "Select or Drop files"}
              </p>
            </div>
          </div>

          <div className="mt-4">
            <div className="flex items-center justify-between mb-2">
              <p className="text-sm text-gray-600">
                Total Files: {files.length}
              </p>

              <button
                onClick={createNewFolder}
                className="text-sm text-blue-600 hover:text-blue-700 px-3 py-1 border border-gray-200 rounded"
              >
                New Folder
              </button>
            </div>

            <div className="mt-4 p-2 border border-gray-200 rounded-lg ">
              {renderTree(files)}
            </div>
          </div>
        </div>
      </DndProvider>
    </ErrorBoundary>
  );
};

export default FileManager;
