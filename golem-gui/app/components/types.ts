export interface FileEntity {
    id: string;
    name: string;
    size: number;
    type: "file" | "folder";
    parentId: string | null;
    isLocked: boolean;
    fileObject?: File;
  }