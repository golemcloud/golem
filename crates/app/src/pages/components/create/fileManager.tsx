"use client";

import * as React from "react";
import {
  ChevronRight,
  File,
  Folder,
  Lock,
  Trash2,
  Unlock,
  Pencil,
  Check,
  X,
} from "lucide-react";
import { useDropzone } from "react-dropzone";
import { useDrag, useDrop } from "react-dnd";

import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { Input } from "@/components/ui/input";

interface FileItem {
  id: string;
  name: string;
  size: number;
  type: "file" | "folder";
  parentId: string | null;
  isLocked: boolean;
}

const ItemTypes = {
  FILE: "file",
  FOLDER: "folder",
};

export function FileManager() {
  const [files, setFiles] = React.useState<FileItem[]>([]);
  const [expandedFolders, setExpandedFolders] = React.useState<Set<string>>(
    new Set()
  );
  const [editingId, setEditingId] = React.useState<string | null>(null);
  const [editingName, setEditingName] = React.useState("");

  const onDrop = React.useCallback((acceptedFiles: File[]) => {
    const newFiles: FileItem[] = acceptedFiles.map((file) => ({
      id: Math.random().toString(36).substring(7),
      name: file.name,
      size: file.size,
      type: "file",
      parentId: null,
      isLocked: false,
    }));
    setFiles((prev) => [...prev, ...newFiles]);
  }, []);

  const { getRootProps, getInputProps, isDragActive } = useDropzone({ onDrop });

  const createFolder = () => {
    const newFolder: FileItem = {
      id: Math.random().toString(36).substring(7),
      name: "New Folder",
      size: 0,
      type: "folder",
      parentId: null,
      isLocked: false,
    };
    setFiles((prev) => [...prev, newFolder]);
    setEditingId(newFolder.id);
    setEditingName("New Folder");
  };

  const startEditing = (file: FileItem) => {
    setEditingId(file.id);
    setEditingName(file.name);
  };

  const saveEditing = () => {
    if (editingId) {
      setFiles((prev) =>
        prev.map((file) =>
          file.id === editingId ? { ...file, name: editingName } : file
        )
      );
      setEditingId(null);
    }
  };

  const cancelEditing = () => {
    setEditingId(null);
  };

  const toggleLock = (fileId: string) => {
    setFiles((prev) =>
      prev.map((file) => {
        if (file.id === fileId || file.parentId === fileId) {
          return { ...file, isLocked: !file.isLocked };
        }
        return file;
      })
    );
  };

  const deleteFile = (fileId: string) => {
    setFiles((prev) =>
      prev.filter((file) => file.id !== fileId && file.parentId !== fileId)
    );
  };

  const moveFile = (fileId: string, targetFolderId: string | null) => {
    const isValidMove = (
      fileId: string,
      targetFolderId: string | null
    ): boolean => {
      if (fileId === targetFolderId) return false;
      const file = files.find((f) => f.id === fileId);
      if (!file) return false;
      if (file.type === "folder") {
        // Prevent moving a folder into its own subfolder
        let currentParent = targetFolderId;
        while (currentParent) {
          if (currentParent === fileId) return false;
          const parent = files.find((f) => f.id === currentParent);
          currentParent = parent ? parent.parentId : null;
        }
      }
      return true;
    };

    if (!isValidMove(fileId, targetFolderId)) return;

    setFiles((prev) =>
      prev.map((file) => {
        if (file.id === fileId) {
          return { ...file, parentId: targetFolderId };
        }
        return file;
      })
    );

    // Expand the target folder if it's not already expanded
    if (targetFolderId) {
      setExpandedFolders((prev) => new Set(prev).add(targetFolderId));
    }
  };

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

  const rootFiles = files.filter((file) => file.parentId === null);
  const getChildFiles = (parentId: string) =>
    files.filter((file) => file.parentId === parentId);

  const DraggableItem = ({ file }: { file: FileItem }) => {
    const isExpanded = expandedFolders.has(file.id);
    const childFiles = file.type === "folder" ? getChildFiles(file.id) : [];

    const [{ isDragging }, drag] = useDrag(() => ({
      type: ItemTypes.FILE,
      item: { id: file.id, type: file.type },
      collect: (monitor) => ({
        isDragging: monitor.isDragging(),
      }),
    }));

    const [{ isOver }, drop] = useDrop(() => ({
      accept: ItemTypes.FILE,
      drop: (item: { id: string; type: string }, monitor) => {
        if (!monitor.isOver({ shallow: true })) return;
        if (item.id !== file.id) {
          moveFile(item.id, file.type === "folder" ? file.id : file.parentId);
        }
      },
      collect: (monitor) => ({
        isOver: monitor.isOver({ shallow: true }),
      }),
    }));

    return (
      <div
        ref={(node) => drag(drop(node))}
        className={`${isDragging ? "opacity-50" : ""} ${
          isOver ? "bg-muted/50" : ""
        }`}
      >
        <div className="flex items-center gap-2 py-1 px-2 rounded-md hover:bg-muted/50">
          {file.type === "folder" && (
            <Button
              variant="ghost"
              size="icon"
              className="h-4 w-4"
              onClick={(e) => {
                e.stopPropagation();
                toggleFolder(file.id);
              }}
            >
              <ChevronRight
                className={`h-4 w-4 transition-transform ${
                  isExpanded ? "rotate-90" : ""
                }`}
              />
            </Button>
          )}
          {file.type === "folder" ? (
            <Folder className="h-4 w-4" />
          ) : (
            <File className="h-4 w-4" />
          )}

          {editingId === file.id ? (
            <div className="flex items-center gap-2 flex-1">
              <Input
                value={editingName}
                onChange={(e) => setEditingName(e.target.value)}
                className="h-7 py-1"
                autoFocus
                onKeyDown={(e) => {
                  if (e.key === "Enter") saveEditing();
                  if (e.key === "Escape") cancelEditing();
                }}
              />
              <Button
                variant="ghost"
                size="icon"
                className="h-6 w-6"
                onClick={saveEditing}
              >
                <Check className="h-4 w-4" />
              </Button>
              <Button
                variant="ghost"
                size="icon"
                className="h-6 w-6"
                onClick={cancelEditing}
              >
                <X className="h-4 w-4" />
              </Button>
            </div>
          ) : (
            <>
              <span className="flex-1">{file.name}</span>
              {file.type === "file" && (
                <span className="text-sm text-muted-foreground">
                  {Math.round(file.size / 1024)} KB
                </span>
              )}
              <Button
                variant="ghost"
                size="icon"
                className="h-8 w-8"
                onClick={(e) => {
                  e.stopPropagation();
                  startEditing(file);
                }}
              >
                <Pencil className="h-4 w-4" />
              </Button>
              <Button
                variant="ghost"
                size="icon"
                className="h-8 w-8"
                onClick={(e) => {
                  e.stopPropagation();
                  toggleLock(file.id);
                }}
              >
                {file.isLocked ? (
                  <Lock className="h-4 w-4" />
                ) : (
                  <Unlock className="h-4 w-4" />
                )}
              </Button>
              <Button
                variant="ghost"
                size="icon"
                className="h-8 w-8"
                onClick={(e) => {
                  e.stopPropagation();
                  deleteFile(file.id);
                }}
              >
                <Trash2 className="h-4 w-4" />
              </Button>
            </>
          )}
        </div>
        {file.type === "folder" && isExpanded && (
          <div className="ml-6">
            {childFiles.map((childFile) => (
              <DraggableItem key={childFile.id} file={childFile} />
            ))}
          </div>
        )}
      </div>
    );
  };

  return (
    <div className="w-full max-w-3xl mx-auto p-4">
      <Card className="p-6">
        <div className="space-y-4">
          <div>
            <h2 className="text-2xl font-bold">Initial Files</h2>
            <p className="text-muted-foreground">
              Files available to your workers at runtime.
            </p>
          </div>
          <div
            {...getRootProps()}
            className={`border-2 border-dashed rounded-lg p-8 text-center ${
              isDragActive ? "border-primary" : "border-muted"
            }`}
          >
            <input {...getInputProps()} />
            <p>Select or Drop files</p>
          </div>
          <div className="flex items-center justify-between">
            <h3 className="text-lg font-semibold">
              Total Files: {files.length}
            </h3>
            <div className="space-x-2">
              <Button onClick={createFolder}>New Folder</Button>
            </div>
          </div>
          <div
            className="space-y-1"
            ref={
              useDrop(() => ({
                accept: ItemTypes.FILE,
                drop: (item: { id: string }, monitor) => {
                  if (monitor.didDrop()) return;
                  moveFile(item.id, null);
                },
              }))[1]
            }
          >
            {rootFiles.map((file) => (
              <DraggableItem key={file.id} file={file} />
            ))}
          </div>
        </div>
      </Card>
    </div>
  );
}
