import { DragDropItem } from "./dragndrop-item";
import { Button2 as Button } from "../ui/button"; 
import { Trash } from "lucide-react";
import { v4 as uuidv4 } from 'uuid';

export interface FileEntity {
  id: string;
  name: string;
  size: number;
  type: "file" | "folder";
  parentId: string | null;
  isLocked: boolean;
  fileObject?: File;
}

export function FileList({
  files,
  setFiles,
  openFolders,
  setOpenFolders,
  editingId,
  setEditingId,
  editingName,
  setEditingName,
}: {
  files: FileEntity[];
  setFiles: React.Dispatch<React.SetStateAction<FileEntity[]>>;
  openFolders: Set<string>;
  setOpenFolders: React.Dispatch<React.SetStateAction<Set<string>>>;
  editingId: string | null;
  setEditingId: React.Dispatch<React.SetStateAction<string | null>>;
  editingName: string;
  setEditingName: React.Dispatch<React.SetStateAction<string>>;
}) {
  const topLevelFiles = files.filter((file) => file.parentId === null);
  const addNewFolder = (e: React.MouseEvent<HTMLButtonElement>) => {
    e.preventDefault();
    e.stopPropagation();
    const newFolder: FileEntity = {
      id:uuidv4(),
      name: "New Folder",
      size: 0,
      type: "folder",
      parentId: null,
      isLocked: false,
    };
    setFiles((prev) => [...prev, newFolder]);
  };

  return (
    <div className="border p-4 rounded-md">
      <div className="flex items-center justify-between">
        <label className="text-sm font-bold leading-none peer-disabled:cursor-not-allowed peer-disabled:opacity-70">
          Total Files: {files.length}
        </label>
        <div className="flex justify-end">
        <Button variant="primary" size="xs" onClick={addNewFolder}>
          New Folder
        </Button>
        <Button variant="error" size="xs" className="ml-2" onClick={(e)=>{
            e.preventDefault();
            e.stopPropagation();
          setFiles([])}
          }>
          <Trash/>
        </Button>
      </div>
      </div>
      <div className="space-y-1 py-2">
        {topLevelFiles.map((file) => (
          <DragDropItem
            key={file.id}
            file={file}
            files={files}
            setFiles={setFiles}
            openFolders={openFolders}
            setOpenFolders={setOpenFolders}
            editingId={editingId}
            setEditingId={setEditingId}
            editingName={editingName}
            setEditingName={setEditingName}
          />
        ))}
      </div>
    </div>
  );
}