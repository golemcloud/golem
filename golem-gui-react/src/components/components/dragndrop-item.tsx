import { useEffect, useRef } from "react";
import { FileEntity } from "../../../../golem-gui/app/components/types";
import { useDrag, useDrop } from "react-dnd";
import { TextField } from "@mui/material";
import { Button2 as Button } from "../ui/button";
import {
  ChevronRight,
  Folder,
  File,
  Lock,
  LockOpen as Unlock,
  Trash,
} from "lucide-react";

export function DragDropItem({
  file,
  files,
  setFiles,
  openFolders,
  setOpenFolders,
  editingId,
  setEditingId,
  editingName,
  setEditingName,
}: {
  file: FileEntity;
  files: FileEntity[];
  setFiles: React.Dispatch<React.SetStateAction<FileEntity[]>>;
  openFolders: Set<string>;
  setOpenFolders: React.Dispatch<React.SetStateAction<Set<string>>>;
  editingId: string | null;
  setEditingId: React.Dispatch<React.SetStateAction<string | null>>;
  editingName: string;
  setEditingName: React.Dispatch<React.SetStateAction<string>>;
}) {
  const isFolderOpen = openFolders.has(file.id);
  const nestedFiles =
    file.type === "folder" ? files.filter((f) => f.parentId === file.id) : [];

  const [{ isDragging }, drag] = useDrag(() => ({
    type: "file",
    item: { id: file.id, type: file.type },
    collect: (monitor) => ({
      isDragging: monitor.isDragging(),
    }),
  }));

  const [{ isOver }, drop] = useDrop(() => ({
    accept: "file",
    drop: (item: { id: string; type: string }, monitor) => {
      if (!monitor.isOver({ shallow: true })) return;
      if (item.id !== file.id) {
        relocateFile(item.id, file.type === "folder" ? file.id : file.parentId);
      }
    },
    collect: (monitor) => ({
      isOver: monitor.isOver({ shallow: true }),
    }),
  }));

  const relocateFile = (fileId: string, targetFolderId: string | null) => {
    const validateMove = (
      fileId: string,
      targetFolderId: string | null
    ): boolean => {
      if (targetFolderId === null) return true;
      if (fileId === targetFolderId) return false;
      let parent = files.find((f) => f.id === targetFolderId) || null;
      while (parent) {
        if (parent.id === fileId) return false;
        parent = parent.parentId
          ? files.find((f) => f.id === parent?.parentId) || null
          : null;
      }
      return true;
    };

    if (!validateMove(fileId, targetFolderId)) return;

    setFiles((prev) =>
      prev.map((file) => {
        if (file.id === fileId) {
          return { ...file, parentId: targetFolderId };
        }
        return file;
      })
    );
    if (targetFolderId) {
      setOpenFolders((prev) => new Set(prev).add(targetFolderId));
    }
  };

  const toggleFolderView = (folderId: string) => {
    setOpenFolders((prev) => {
      const next = new Set(prev);
      if (next.has(folderId)) {
        next.delete(folderId);
      } else {
        next.add(folderId);
      }
      return next;
    });
  };

  const initiateEdit = (file: FileEntity) => {
    setEditingId(file.id);
    setEditingName(file.name);
  };

  const finalizeEdit = () => {
    if (editingId) {
      setFiles((prev) =>
        prev.map((file) =>
          file.id === editingId ? { ...file, name: editingName } : file
        )
      );
      setEditingId(null);
    }
  };

  const cancelEdit = () => {
    setEditingId(null);
  };

  const toggleFileLock = (fileId: string) => {
    setFiles((prev) =>
      prev.map((file) => {
        if (file.id === fileId || file.parentId === fileId) {
          return { ...file, isLocked: !file.isLocked };
        }
        return file;
      })
    );
  };

  const removeFile = (fileId: string) => {
    setFiles((prev) =>
      prev.filter((file) => file.id !== fileId && file.parentId !== fileId)
    );
  };

  // Ref for the input field
  const inputRef = useRef<HTMLInputElement>(null);

  // Handle clicks outside the input field
  useEffect(() => {
    const handleClickOutside = (event: MouseEvent) => {
      if (
        editingId === file.id &&
        inputRef.current &&
        !inputRef.current.contains(event.target as Node)
      ) {
        finalizeEdit();
      }
    };

    document.addEventListener("mousedown", handleClickOutside);
    return () => {
      document.removeEventListener("mousedown", handleClickOutside);
    };
  }, [editingId, file.id,finalizeEdit]);

  return (
    <div
    // @ts-expect-error - The structure of `ref` is not fully typed yet
      ref={(node) => drag(drop(node))}
      className={`${isDragging ? "opacity-50" : ""} ${
        isOver ? "bg-muted/50" : ""
      }`}
    >
      <div className='flex items-center gap-2 py-1 px-2 rounded-md hover:bg-muted/50'>
        {file.type === "folder" && (
          <Button
            variant='ghost'
            size='icon_sm'
            onClick={(e) => {
              e.preventDefault();
              e.stopPropagation();
              toggleFolderView(file.id);
            }}
          >
            <ChevronRight
              className={`h-4 w-4 transition-transform ${
                isFolderOpen ? "rotate-90" : ""
              }`}
            />
          </Button>
        )}
        {file.type === "folder" ? (
          <Folder className='h-4 w-4' />
        ) : (
          <File className='h-4 w-4' />
        )}

        {editingId === file.id ? (
          <div className='flex items-center gap-2 flex-1'>
            <TextField
              id={`input-${file.id}`}
              value={editingName}
              onChange={(e) => setEditingName(e.target.value)}
              size='small'
              autoFocus
              onKeyDown={(e) => {
                if (e.key === "Enter") finalizeEdit();
                if (e.key === "Escape") cancelEdit();
              }}
              inputRef={inputRef} // Attach the ref to the input field
            />
          </div>
        ) : (
          <>
            <span className='flex-1 text-xs' onDoubleClick={() => initiateEdit(file)}>
              {file.name}
            </span>
            {file.type === "file" && (
              <span className='text-xs text-muted-foreground'>
                {Math.round(file.size / 1024)} KB
              </span>
            )}
          </>
        )}
        <Button
          variant='ghost'
          size='icon_sm'
          onClick={(e) => {
            e.preventDefault();
            e.stopPropagation();
            toggleFileLock(file.id);
          }}
        >
          {file.isLocked ? (
            <Lock className='h-4 w-4' />
          ) : (
            <Unlock className='h-4 w-4' />
          )}
        </Button>
        <Button
          variant='error'
          size='xs'
          onClick={(e) => {
            e.stopPropagation();
            removeFile(file.id);
          }}
        >
          <Trash className='h-4 w-4' />
        </Button>
      </div>
      {file.type === "folder" && isFolderOpen && (
        <div className='ml-6'>
          {nestedFiles.map((childFile) => (
            <DragDropItem
              key={childFile.id}
              file={childFile}
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
      )}
    </div>
  );
}