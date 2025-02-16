import {
  ChevronDown,
  ChevronRight,
  File,
  Folder,
  FolderOpen,
  FolderPlus,
  Lock,
  LockOpen,
  MoveLeft,
  MoveRight,
  Pencil,
  Plus,
  Upload,
  X
} from 'lucide-react';
import { DragEvent, useRef, useState } from 'react';

interface FileItem {
  id: string;
  name: string;
  type: "file" | "folder";
  parentId: string | null;
  fileObject?: File;
  isLocked?: boolean;
  path?: string;
}

interface FileSystemManagerProps {
  files: FileItem[];
  onFilesChange: (files: FileItem[]) => void;
  isSubmitting: boolean;
  accept?: string;
}

const FileUploadZone = ({ onUpload, isSubmitting, accept = "*" }) => {
  const [dragActive, setDragActive] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);

  const handleDrop = (e: DragEvent) => {
    e.preventDefault();
    setDragActive(false);
    if (e.dataTransfer.files.length) {
      onUpload(e.dataTransfer.files);
    }
  };

  return (
    <div
      onClick={() => !isSubmitting && inputRef.current?.click()}
      onDragOver={(e) => {
        e.preventDefault();
        !isSubmitting && setDragActive(true);
      }}
      onDragLeave={() => setDragActive(false)}
      onDrop={handleDrop}
      className={`border-2 border-dashed rounded-lg p-6 transition-all duration-200
        ${isSubmitting ? "cursor-not-allowed opacity-60" : "cursor-pointer hover:border-primary"} 
        ${dragActive ? "border-primary bg-primary/10" : "border-border"}
        flex flex-col items-center justify-center gap-4`}
    >
      <div className="p-4 rounded-full bg-primary/10">
        <Upload className="h-6 w-6 text-primary" />
      </div>
      <div className="text-center">
        <p className="text-sm font-medium">Drag files here or click to upload</p>
        <p className="text-xs text-muted-foreground mt-1">
          Files will appear in the file system below
        </p>
      </div>
      <input
        ref={inputRef}
        type="file"
        accept={accept}
        multiple
        onChange={(e) => e.target.files && onUpload(e.target.files)}
        className="hidden"
        disabled={isSubmitting}
      />
    </div>
  );
};


interface DragItem {
  id: string;
  type: "file" | "folder";
}

const FileTreeItem = ({
  item,
  files,
  onMove,
  onRename,
  onDelete,
  onToggleLock,
  onCreateFolder,
  level = 0,
}: {
  item: FileItem;
  files: FileItem[];
  onMove: (itemId: string, newParentId: string | null) => void;
  onRename: (itemId: string, newName: string) => void;
  onDelete: (itemId: string) => void;
  onToggleLock: (itemId: string) => void;
  onCreateFolder: (name: string, parentId: string | null) => void;
  level?: number;
}) => {
  const [isExpanded, setIsExpanded] = useState(true);
  const [isEditing, setIsEditing] = useState(false);
  const [editName, setEditName] = useState(item.name);
  const [isDragOver, setIsDragOver] = useState(false);

  const children = files.filter(f => f.parentId === item.id);
  const canMoveUp = item.parentId !== null;
  const canMoveIn = level > 0;

  const handleRename = () => {
    if (editName.trim() && editName !== item.name) {
      onRename(item.id, editName.trim());
    }
    setIsEditing(false);
  };

  const handleDragStart = (e: DragEvent) => {
    const dragData: DragItem = {
      id: item.id,
      type: item.type
    };
    e.dataTransfer.setData('application/json', JSON.stringify(dragData));
    e.dataTransfer.effectAllowed = 'move';
  };

  const handleDragOver = (e: DragEvent) => {
    e.preventDefault();
    e.stopPropagation();
    if (item.type === 'folder') {
      setIsDragOver(true);
      e.dataTransfer.dropEffect = 'move';
    }
  };

  const handleDragLeave = (e: DragEvent) => {
    e.preventDefault();
    e.stopPropagation();
    setIsDragOver(false);
  };

  const handleDrop = (e: DragEvent) => {
    e.preventDefault();
    e.stopPropagation();
    setIsDragOver(false);

    if (item.type !== 'folder') return;

    try {
      const dragData: DragItem = JSON.parse(
        e.dataTransfer.getData('application/json')
      );

      // Prevent dropping on itself or into its own child folders
      if (dragData.id === item.id) return;

      const draggedItem = files.find(f => f.id === dragData.id);
      if (!draggedItem) return;

      // Check if target is not a child of the dragged folder
      const isChildOfDragged = (parentId: string | null): boolean => {
        if (!parentId) return false;
        if (parentId === dragData.id) return true;
        const parent = files.find(f => f.id === parentId);
        return parent ? isChildOfDragged(parent.parentId) : false;
      };

      if (!isChildOfDragged(item.id)) {
        onMove(dragData.id, item.id);
      }
    } catch (error) {
      console.error('Error processing drop:', error);
    }
  };

  return (
    <div className="w-full">
      <div
        draggable
        onDragStart={handleDragStart}
        onDragOver={handleDragOver}
        onDragLeave={handleDragLeave}
        onDrop={handleDrop}
        className={`group flex items-center gap-2 p-2 rounded-lg transition-colors
          ${isDragOver ? 'bg-primary/10 border-2 border-dashed border-primary' : 'hover:bg-card/60'}`}
        style={{ paddingLeft: `${level * 1.5 + 0.5}rem` }}
      >
        <div className="flex items-center gap-2 flex-1 min-w-0">
          {item.type === 'folder' && (
            <button
              onClick={() => setIsExpanded(!isExpanded)}
              className="text-primary hover:text-primary/80"
            >
              {isExpanded ? <ChevronDown size={16} /> : <ChevronRight size={16} />}
            </button>
          )}
          {item.type === 'folder' ? (
            isExpanded ? <FolderOpen size={16} className="text-primary" />
              : <Folder size={16} className="text-primary" />
          ) : (
            <File size={16} className="text-primary" />
          )}

          {isEditing ? (
            <input
              type="text"
              value={editName}
              onChange={(e) => setEditName(e.target.value)}
              onBlur={handleRename}
              onKeyDown={(e) => {
                if (e.key === 'Enter') handleRename();
                if (e.key === 'Escape') {
                  setEditName(item.name);
                  setIsEditing(false);
                }
              }}
              className="flex-1 min-w-0 px-2 py-1 text-sm bg-card rounded border border-input focus:border-primary"
              autoFocus
            />
          ) : (
            <span
              className="flex-1 text-sm truncate cursor-pointer"
              onDoubleClick={() => setIsEditing(true)}
            >
              {item.name}
            </span>
          )}
        </div>

        <div className="flex items-center gap-1 opacity-0 group-hover:opacity-100 transition-opacity">
          {canMoveIn && (
            <button
              onClick={() => onMove(item.id, null)}
              className="p-1 text-muted-foreground hover:text-primary rounded-md transition-colors"
              title="Move out of folder"
            >
              <MoveLeft size={14} />
            </button>
          )}
          {canMoveUp && (
            <button
              onClick={() => {
                const currentParent = files.find(f => f.id === item.parentId);
                if (currentParent?.parentId !== undefined) {
                  onMove(item.id, currentParent.parentId);
                }
              }}
              className="p-1 text-muted-foreground hover:text-primary rounded-md transition-colors"
              title="Move up one level"
            >
              <MoveRight size={14} />
            </button>
          )}
          {item.type === 'folder' && (
            <>
              <button
                onClick={() => onCreateFolder('New Folder', item.id)}
                className="p-1 text-muted-foreground hover:text-primary rounded-md transition-colors"
                title="Create subfolder"
              >
                <FolderPlus size={14} />
              </button>
              <button
                onClick={() => setIsEditing(true)}
                className="p-1 text-muted-foreground hover:text-primary rounded-md transition-colors"
                title="Rename folder"
              >
                <Pencil size={14} />
              </button>
            </>
          )}
          {item.type === 'file' && (
            <button
              onClick={() => onToggleLock(item.id)}
              className="p-1 text-muted-foreground hover:text-primary rounded-md transition-colors"
              title={item.isLocked ? "Unlock file" : "Lock file"}
            >
              {item.isLocked ? <Lock size={14} /> : <LockOpen size={14} />}
            </button>
          )}
          <button
            onClick={() => onDelete(item.id)}
            className="p-1 text-muted-foreground hover:text-destructive rounded-md transition-colors"
            title="Delete"
          >
            <X size={14} />
          </button>
        </div>
      </div>

      {item.type === 'folder' && isExpanded && (
        <div className="ml-4 border-l border-border">
          {children.map(child => (
            <FileTreeItem
              key={child.id}
              item={child}
              files={files}
              onMove={onMove}
              onRename={onRename}
              onDelete={onDelete}
              onToggleLock={onToggleLock}
              onCreateFolder={onCreateFolder}
              level={level + 1}
            />
          ))}
        </div>
      )}
    </div>
  );
};

const FileSystemManager = ({
  files,
  onFilesChange,
  isSubmitting,
  accept
}: FileSystemManagerProps) => {
  const rootFiles = files.filter(f => f.parentId === null);

  const handleUpload = (fileList: FileList) => {
    const newFiles = Array.from(fileList).map((file) => ({
      id: Math.random().toString(36).substring(7),
      name: file.name,
      type: "file" as const,
      parentId: null,
      fileObject: file,
      isLocked: false
    }));
    onFilesChange([...files, ...newFiles]);
  };

  const handleCreateFolder = (name: string, parentId: string | null = null) => {
    const newFolder = {
      id: Math.random().toString(36).substring(7),
      name,
      type: "folder" as const,
      parentId,
    };
    onFilesChange([...files, newFolder]);
  };

  const handleMove = (itemId: string, newParentId: string | null) => {
    onFilesChange(files.map(file =>
      file.id === itemId ? { ...file, parentId: newParentId } : file
    ));
  };

  const handleRename = (itemId: string, newName: string) => {
    onFilesChange(files.map(file =>
      file.id === itemId ? { ...file, name: newName } : file
    ));
  };

  const handleDelete = (itemId: string) => {
    const itemToDelete = files.find(f => f.id === itemId);
    if (itemToDelete?.type === 'folder') {
      // Recursively delete all children
      const getChildrenIds = (parentId: string): string[] => {
        const children = files.filter(f => f.parentId === parentId);
        return children.reduce((acc, child) => [
          ...acc,
          child.id,
          ...(child.type === 'folder' ? getChildrenIds(child.id) : [])
        ], [] as string[]);
      };
      const idsToDelete = [itemId, ...getChildrenIds(itemId)];
      onFilesChange(files.filter(f => !idsToDelete.includes(f.id)));
    } else {
      onFilesChange(files.filter(f => f.id !== itemId));
    }
  };

  const handleToggleLock = (itemId: string) => {
    onFilesChange(files.map(file =>
      file.id === itemId ? { ...file, isLocked: !file.isLocked } : file
    ));
  };

  return (
    <div className="space-y-4">
      <FileUploadZone
        onUpload={handleUpload}
        isSubmitting={isSubmitting}
        accept={accept}
      />

      {rootFiles.length > 0 &&
        <div className="border rounded-lg">
          <div className="p-4 border-b flex items-center justify-between">
            <h3 className="text-sm font-medium flex items-center gap-2">
              <Folder size={16} className="text-primary" />
              File System
            </h3>
            <button
              onClick={() => handleCreateFolder('New Folder')}
              className="flex items-center gap-2 text-xs px-2 py-1 rounded-md bg-primary/10 text-primary hover:bg-primary/20 transition-colors"
            >
              <Plus size={14} />
              New Folder
            </button>
          </div>

          <div className="p-2">
            {rootFiles.length > 0 ? (
              rootFiles.map(file => (
                <FileTreeItem
                  key={file.id}
                  item={file}
                  files={files}
                  onMove={handleMove}
                  onRename={handleRename}
                  onDelete={handleDelete}
                  onToggleLock={handleToggleLock}
                  onCreateFolder={handleCreateFolder}
                />
              ))
            ) : (
              <div className="text-center py-8 text-muted-foreground text-sm">
                No files or folders yet
              </div>
            )}
          </div>
        </div>}
    </div>
  );
};

export default FileSystemManager;