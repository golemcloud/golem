import {
  AlertCircle,
  Cloud,
  FileIcon,
  Folder,
  Loader2,
  Plus,
  Server,
  Upload,
  X,
} from "lucide-react";
import { useCreateComponent, useUpdateComponent } from "../../api/components";
import { useEffect, useRef, useState } from "react";

import { Component } from "../../types/api";
import FileSystemManager from "./FileSystemManager";
import HierarchicalFileDropzone from "./FileSystemManager";
import JSZip from "jszip";
import toast from "react-hot-toast";

type ComponentType = "Durable" | "Ephemeral";

interface ComponentModalProps {
  isOpen: boolean;
  onClose: () => void;
  existingComponent?: Component;
}

interface FileItem {
  id: string;
  name: string;
  type: "file" | "folder";
  parentId: string | null;
  fileObject?: File;
  isLocked?: boolean;
  path?: string;
}

const FileDropzone = ({
  onDrop,
  files,
  onRemove,
  isSubmitting,
  placeholder,
  accept = "*",
}: {
  onDrop: (files: FileList) => void;
  files: FileItem[];
  onRemove: (id: string) => void;
  isSubmitting: boolean;
  placeholder: string;
  accept?: string;
}) => {
  const [dragActive, setDragActive] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);

  const handleDrop = (e: React.DragEvent) => {
    e.preventDefault();
    setDragActive(false);
    if (e.dataTransfer.files.length) {
      onDrop(e.dataTransfer.files);
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
      className={`border-2 border-dashed rounded-lg p-6 text-center transition-all duration-200
        ${isSubmitting ? "cursor-not-allowed opacity-60" : "cursor-pointer hover:border-primary"} 
        ${dragActive ? "border-primary bg-primary/10" : "border-muted"}`}
    >
      {files.length > 0 ? (
        <div className="space-y-2">
          {files.map((file) => (
            <div
              key={file.id}
              className="flex items-center justify-between bg-card/50 rounded-lg px-4 py-2"
            >
              <div className="flex items-center gap-2">
                {file.type === "folder" ? (
                  <Folder size={16} className="text-primary" />
                ) : (
                  <FileIcon size={16} className="text-primary" />
                )}
                <span className="text-sm truncate">{file.name}</span>
              </div>
              {!isSubmitting && (
                <button
                  onClick={(e) => {
                    e.stopPropagation();
                    onRemove(file.id);
                  }}
                  className="p-1 text-muted-foreground hover:text-destructive rounded-md
                    hover:bg-destructive/10 transition-colors"
                >
                  <X size={14} />
                </button>
              )}
            </div>
          ))}
        </div>
      ) : (
        <div className="space-y-3">
          <Upload className="h-8 w-8 mx-auto text-muted-foreground" />
          <div>
            <p className="text-sm text-foreground">{placeholder}</p>
            <p className="text-xs text-muted-foreground mt-1">
              or click to browse
            </p>
          </div>
        </div>
      )}
      <input
        ref={inputRef}
        type="file"
        accept={accept}
        multiple
        onChange={(e) => e.target.files && onDrop(e.target.files)}
        className="hidden"
        disabled={isSubmitting}
      />
    </div>
  );
};

const CreateComponentModal = ({
  isOpen,
  onClose,
  existingComponent,
}: ComponentModalProps) => {
  const isUpdateMode = !!existingComponent;
  const [name, setName] = useState("");
  const [componentType, setComponentType] = useState<ComponentType>("Durable");
  const [mainFile, setMainFile] = useState<File | null>(null);
  const [files, setFiles] = useState<FileItem[]>([]);
  const [isSubmitting, setIsSubmitting] = useState(false);

  const createComponent = useCreateComponent();
  const updateComponent = useUpdateComponent();

  useEffect(() => {
    if (existingComponent) {
      setName(existingComponent.componentName);
      setComponentType(existingComponent.componentType);
    }
  }, [existingComponent]);

  // Function to get the full path of a file
  const getFullPath = (file: FileItem, allFiles: FileItem[]): string => {
    if (!file.parentId) return `/${file.name}`;
    const parent = allFiles.find((f) => f.id === file.parentId);
    if (!parent) return file.name;
    return `${getFullPath(parent, allFiles)}/${file.name}`;
  };

  // Function to capture file metadata
  const captureFileMetadata = (allFiles: FileItem[]) => {
    const filesPath: { path: string; permissions: string }[] = [];
    allFiles.forEach((file) => {
      if (file.type !== "folder") {
        filesPath.push({
          path: getFullPath(file, allFiles),
          permissions: file.isLocked ? "read-only" : "read-write",
        });
      }
    });
    return { values: filesPath };
  };

  // Function to add files to zip
  const addFilesToZip = async (zipFolder: JSZip, parentId: string | null) => {
    const children = files.filter((file) => file.parentId === parentId);
    for (const child of children) {
      if (child.type === "folder") {
        const folder = zipFolder.folder(child.name);
        if (folder) {
          await addFilesToZip(folder, child.id);
        }
      } else if (child.type === "file" && child.fileObject) {
        zipFolder.file(child.name, child.fileObject);
      }
    }
  };

  const handleMainFileDrop = (fileList: FileList) => {
    const file = fileList[0];
    if (file?.name.endsWith(".wasm")) {
      setMainFile(file);
    } else {
      toast.error("Please upload a .wasm file");
    }
  };

  const handleAdditionalFiles = (fileList: FileList, parentId: string | null = null) => {
    const newFiles = Array.from(fileList).map((file) => ({
      id: Math.random().toString(36).substring(7),
      name: file.name,
      type: "file" as const,
      parentId,
      fileObject: file,
      isLocked: false
    }));
    setFiles((prev) => [...prev, ...newFiles]);
  };

  const handleToggleLock = (id: string) => {
    setFiles(prev => prev.map(file =>
      file.id === id ? { ...file, isLocked: !file.isLocked } : file
    ));
  };

  const handleCreateFolder = (name: string, parentId: string | null = null) => {
    const newFolder = {
      id: Math.random().toString(36).substring(7),
      name,
      type: "folder" as const,
      parentId,
    };
    setFiles(prev => [...prev, newFolder]);
  };

  const removeFile = (id: string) => {
    setFiles((prev) => prev.filter((file) => file.id !== id));
  };

  const handleSubmit = async () => {
    if (!name || (!mainFile && !isUpdateMode)) return;

    setIsSubmitting(true);
    const formData = new FormData();
    formData.append("name", name);
    formData.append("componentType", componentType);

    if (mainFile) {
      formData.append("component", mainFile);
    }

    try {
      // Create zip file containing additional files
      if (files.length > 0) {
        const zip = new JSZip();

        const addFileToZip = async (file: FileItem, parentPath: string = '') => {
          const filePath = parentPath ? `${parentPath}/${file.name}` : file.name;

          if (file.type === 'folder') {
            const folder = zip.folder(filePath);
            const children = files.filter(f => f.parentId === file.id);
            for (const child of children) {
              await addFileToZip(child, filePath);
            }
          } else if (file.fileObject) {
            zip.file(filePath, file.fileObject);
          }
        };

        // First add all root level files
        const rootFiles = files.filter(f => f.parentId === null);
        for (const file of rootFiles) {
          await addFileToZip(file);
        }

        // Generate the zip file
        const blob = await zip.generateAsync({ type: "blob" });
        formData.append("files", blob, "files.zip");

        // Create file permissions metadata
        const filePermissions = files
          .filter(file => file.type === 'file')
          .map(file => {
            const getFilePath = (fileItem: FileItem): string => {
              const parent = files.find(f => f.id === fileItem.parentId);
              if (!parent) return fileItem.name;
              return `/${getFilePath(parent)}/${fileItem.name}`;
            };

            return {
              path: getFilePath(file),
              permissions: file.isLocked ? "read-only" : "read-write"
            };
          });

        formData.append(
          "filesPermissions",
          JSON.stringify({ values: filePermissions })
        );
      }

      if (isUpdateMode && existingComponent) {
        await updateComponent.mutateAsync({
          componentId: existingComponent.versionedComponentId.componentId,
          formData,
        });
        toast.success("Component updated successfully");
      } else {
        await createComponent.mutateAsync(formData);
        toast.success("Component created successfully");
      }

      setMainFile(null);
      setFiles([]);
      setName("");
      setComponentType("Durable");
      setIsSubmitting(false);
      onClose();
    } catch (error) {
      toast.error(`Failed to ${isUpdateMode ? "update" : "create"} component`);
      setIsSubmitting(false);
      console.error(error);
    }
  };

  if (!isOpen) return null;

  return (
    <div className="fixed inset-0 bg-background/60 flex items-center justify-center p-4 z-50 backdrop-blur-sm">
      <div className="bg-card rounded-xl p-6 max-w-xl w-full shadow-xl">
        <div className="flex justify-between items-start mb-6">
          <div className="flex items-center gap-3">
            <div className="p-2 rounded-md bg-primary/10 text-primary">
              <Plus size={20} />
            </div>
            <div>
              <h2 className="text-xl font-semibold">
                {isUpdateMode ? "Update Component" : "Create New Component"}
              </h2>
              <p className="text-sm text-muted-foreground mt-1">
                Configure your component settings
              </p>
            </div>
          </div>
          <button
            onClick={onClose}
            className="text-muted-foreground hover:text-foreground p-1 hover:bg-muted/50 
              rounded-md transition-colors"
          >
            <X size={20} />
          </button>
        </div>

        <div className="space-y-6">
          <div>
            <label className="block text-sm font-medium mb-1.5">
              Component Name
            </label>
            <input
              type="text"
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder="Enter component name"
              disabled={isSubmitting || isUpdateMode}
              className="w-full px-4 py-2.5 bg-card/50 rounded-lg border border-input 
                focus:border-primary focus:ring-1 focus:ring-primary outline-none
                transition-all duration-200 disabled:opacity-50 disabled:cursor-not-allowed"
            />
          </div>

          <div>
            <label className="block text-sm font-medium mb-1.5">
              Component Type
            </label>
            <div className="grid grid-cols-2 gap-4">
              {[
                { value: "Durable", label: "Durable", icon: Server },
                { value: "Ephemeral", label: "Ephemeral", icon: Cloud },
              ].map((option) => (
                <button
                  key={option.value}
                  onClick={() =>
                    setComponentType(option.value as ComponentType)
                  }
                  className={`flex items-center gap-3 p-4 rounded-lg border-2 transition-all
                    ${componentType === option.value
                      ? "border-primary bg-primary/10"
                      : "border-input hover:border-muted"
                    }`}
                  disabled={isSubmitting}
                >
                  <option.icon
                    className={
                      componentType === option.value
                        ? "text-primary"
                        : "text-muted-foreground"
                    }
                    size={20}
                  />
                  <span>{option.label}</span>
                </button>
              ))}
            </div>
          </div>

          <div className="space-y-4">
            <div>
              <label className="block text-sm font-medium mb-1.5">
                WASM File
              </label>
              <FileDropzone
                onDrop={handleMainFileDrop}
                files={
                  mainFile
                    ? [
                      {
                        id: "main",
                        name: mainFile.name,
                        type: "file",
                        parentId: null,
                        fileObject: mainFile,
                      },
                    ]
                    : []
                }
                onRemove={() => setMainFile(null)}
                isSubmitting={isSubmitting}
                accept=".wasm"
                placeholder="Drag and drop your WASM file here"
              />
            </div>

            <div>
              <label className="block text-sm font-medium mb-1.5">
                Additional Files
              </label>
              <FileSystemManager
                files={files}
                onFilesChange={setFiles}
                isSubmitting={isSubmitting}
              />
            </div>
          </div>

          <div className="flex justify-end items-center gap-3 pt-2">
            <button
              onClick={onClose}
              className="px-4 py-2 text-sm bg-card hover:bg-muted 
                transition-colors disabled:opacity-50 rounded-lg"
              disabled={isSubmitting}
            >
              Cancel
            </button>
            <button
              onClick={handleSubmit}
              disabled={!name || (!mainFile && !isUpdateMode) || isSubmitting}
              className="px-4 py-2 text-sm bg-primary text-primary-foreground rounded-lg hover:bg-primary/90
                disabled:opacity-50 transition-colors flex items-center gap-2"
            >
              {isSubmitting ? (
                <>
                  <Loader2 size={16} className="animate-spin" />
                  <span>{isUpdateMode ? "Updating..." : "Creating..."}</span>
                </>
              ) : (
                <>
                  <Plus size={16} />
                  <span>
                    {isUpdateMode ? "Update Component" : "Create Component"}
                  </span>
                </>
              )}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
};

export default CreateComponentModal;
