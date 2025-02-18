interface File {
    path: string;
    key: string;
    permissions: string;
  }
  
  interface FileNode {
    name: string;
    type: 'folder' | 'file';
    children?: FileNode[];
    key?: string;
    permissions?: string;
  }
  
  export const buildFileTree = (files: File[]): FileNode => {
    const root: FileNode = { name: "root", type: "folder", children: [] };
  
    files.forEach((file) => {
      const pathParts = file.path.split("/").filter((part) => part !== ""); // Split path into parts
      let currentLevel = root.children;
  
      // Traverse or create folders based on the path
      for (let i = 0; i < pathParts.length - 1; i++) {
        const part = pathParts[i];
        let folder = currentLevel?.find((item) => item.name === part && item.type === "folder");
  
        if (!folder) {
          folder = { name: part, type: "folder", children: [] };
          currentLevel?.push(folder);
        }
  
        currentLevel = folder.children;
      }
  
      // Add the file to the last folder
      const fileName = pathParts[pathParts.length - 1];
      currentLevel?.push({
        name: fileName,
        type: "file",
        key: file.key,
        permissions: file.permissions,
      });
    });
  
    return root;
  };
  