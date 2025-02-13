"use client";

import * as React from "react";
import { FileEntity } from "./types";
import { FileDropzone } from "./file-dropzone";
import { FileList } from "./file-list";
import { DndProvider } from "react-dnd";
import { HTML5Backend } from "react-dnd-html5-backend";

export function FileOrganizer({
  files = [],
  setFiles,
}: {
  files: FileEntity[];
  setFiles: React.Dispatch<React.SetStateAction<FileEntity[]>>;
}) {
  const [openFolders, setOpenFolders] = React.useState<Set<string>>(new Set());
  const [editingId, setEditingId] = React.useState<string | null>(null);
  const [editingName, setEditingName] = React.useState("");

  const handleFileDrop = React.useCallback((acceptedFiles: File[]) => {
    const newFiles: FileEntity[] = acceptedFiles.map((file) => ({
      id: Math.random().toString(36).substring(7),
      name: file.name,
      size: file.size,
      type: "file",
      parentId: null,
      isLocked: false,
      fileObject: file,
    }));
    setFiles((prev) => [...prev, ...newFiles]);
  }, [setFiles]);

  return (
    <DndProvider backend={HTML5Backend}>
      <div className="space-y-4">
        <FileDropzone onDrop={handleFileDrop} />
        <FileList
          files={files}
          setFiles={setFiles}
          openFolders={openFolders}
          setOpenFolders={setOpenFolders}
          editingId={editingId}
          setEditingId={setEditingId}
          editingName={editingName}
          setEditingName={setEditingName}
        />
      </div>
    </DndProvider>
  );
}