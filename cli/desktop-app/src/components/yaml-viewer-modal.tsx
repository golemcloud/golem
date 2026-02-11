import { useState, useEffect } from "react";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { YamlEditor } from "@/components/yaml-editor";
import { FileTree, FileTreeNode } from "@/components/file-tree";
import { Save, Loader2 } from "lucide-react";
import { toast } from "@/hooks/use-toast";
import { API } from "@/service";
import { AppYamlFiles, YamlFile } from "@/types/yaml-files";

interface YamlViewerModalProps {
  isOpen: boolean;
  onOpenChange: (open: boolean) => void;
  appId: string;
}

export function YamlViewerModal({
  isOpen,
  onOpenChange,
  appId,
}: YamlViewerModalProps) {
  const [yamlFiles, setYamlFiles] = useState<AppYamlFiles | null>(null);
  const [isLoading, setIsLoading] = useState(false);
  const [selectedFileId, setSelectedFileId] = useState<string>("");
  const [editedContents, setEditedContents] = useState<Record<string, string>>(
    {},
  );
  const [savingFiles, setSavingFiles] = useState<Set<string>>(new Set());

  useEffect(() => {
    if (isOpen && appId) {
      setIsLoading(true);
      API.manifestService
        .getAllAppYamlFiles(appId)
        .then(files => {
          setYamlFiles(files);
          // Set the first available file as selected
          if (files.root) {
            setSelectedFileId("root");
          } else if (files.common.length > 0) {
            setSelectedFileId(`common-${0}`);
          } else if (files.components.length > 0) {
            setSelectedFileId(`component-${0}`);
          }
        })
        .catch(error => {
          toast({
            title: "Failed to Load YAML Files",
            description: String(error),
            variant: "destructive",
          });
        })
        .finally(() => {
          setIsLoading(false);
        });
    }
  }, [appId, isOpen]);

  const getSelectedFile = (): YamlFile | null => {
    if (!yamlFiles || !selectedFileId) return null;

    if (selectedFileId === "root" && yamlFiles.root) {
      return yamlFiles.root;
    }

    if (selectedFileId.startsWith("common-")) {
      const index = parseInt(selectedFileId.replace("common-", ""));
      return yamlFiles.common[index] || null;
    }

    if (selectedFileId.startsWith("component-")) {
      const index = parseInt(selectedFileId.replace("component-", ""));
      return yamlFiles.components[index] || null;
    }

    return null;
  };

  const getFileContent = (file: YamlFile): string => {
    return editedContents[file.path] ?? file.content;
  };

  const handleContentChange = (content: string) => {
    const selectedFile = getSelectedFile();
    if (selectedFile) {
      setEditedContents(prev => ({
        ...prev,
        [selectedFile.path]: content,
      }));
    }
  };

  const handleSave = async (file: YamlFile) => {
    const content = editedContents[file.path];
    if (!content) return;

    setSavingFiles(prev => new Set([...prev, file.path]));

    try {
      await API.manifestService.saveYamlFile(file.path, content);

      // Update the original content and clear edited state
      setYamlFiles(prev => {
        if (!prev) return prev;

        const updateFile = (f: YamlFile) =>
          f.path === file.path ? { ...f, content } : f;

        return {
          ...prev,
          root:
            prev.root && prev.root.path === file.path
              ? { ...prev.root, content }
              : prev.root,
          common: prev.common.map(updateFile),
          components: prev.components.map(updateFile),
        };
      });

      setEditedContents(prev => {
        const newContents = { ...prev };
        delete newContents[file.path];
        return newContents;
      });

      toast({
        title: "YAML Saved",
        description: `${file.name} has been saved successfully.`,
      });
    } catch (error) {
      toast({
        title: "Save Failed",
        description: String(error),
        variant: "destructive",
      });
    } finally {
      setSavingFiles(prev => {
        const newSet = new Set(prev);
        newSet.delete(file.path);
        return newSet;
      });
    }
  };

  const buildFileTree = (): FileTreeNode[] => {
    if (!yamlFiles) return [];

    const tree: FileTreeNode[] = [];

    // Add root file
    if (yamlFiles.root) {
      tree.push({
        id: "root",
        name: yamlFiles.root.name,
        type: "file",
        data: yamlFiles.root,
      });
    }

    // Group common files by their parent folder
    if (yamlFiles.common.length > 0) {
      const commonGroups: Record<string, YamlFile[]> = {};
      yamlFiles.common.forEach(file => {
        // Extract folder name from path like "common-xxx/golem.yaml"
        const match = file.name.match(/^(common-[^/]+)\/(.*)/);
        if (match) {
          const folderName = match[1]!;
          if (!commonGroups[folderName]) {
            commonGroups[folderName] = [];
          }
          commonGroups[folderName].push(file);
        }
      });

      // Create folder nodes for each common folder
      Object.entries(commonGroups).forEach(([folderName, files]) => {
        tree.push({
          id: `common-${folderName}`,
          name: folderName,
          type: "folder",
          children: files.map((file, index) => ({
            id: `common-${folderName}-${index}`,
            name: file.name.split("/").pop() || "golem.yaml",
            type: "file" as const,
            data: file,
          })),
        });
      });
    }

    // Group component files by their parent folders (components-xxx/yyy/golem.yaml)
    if (yamlFiles.components.length > 0) {
      const componentGroups: Record<string, Record<string, YamlFile[]>> = {};

      yamlFiles.components.forEach(file => {
        // Extract paths like "components-xxx/yyy/golem.yaml"
        const match = file.name.match(/^(components-[^/]+)\/([^/]+)\/(.*)/);
        if (match) {
          const topFolder = match[1] as string;
          const subFolder = match[2] as string;

          if (!componentGroups[topFolder]) {
            componentGroups[topFolder] = {};
          }
          if (!componentGroups[topFolder][subFolder]) {
            componentGroups[topFolder][subFolder] = [];
          }
          componentGroups[topFolder][subFolder].push(file);
        }
      });

      // Create nested folder structure
      Object.entries(componentGroups).forEach(([topFolder, subFolders]) => {
        tree.push({
          id: topFolder,
          name: topFolder,
          type: "folder",
          children: Object.entries(subFolders).map(([subFolder, files]) => ({
            id: `${topFolder}-${subFolder}`,
            name: subFolder,
            type: "folder" as const,
            children: files.map((file, index) => ({
              id: `component-${topFolder}-${subFolder}-${index}`,
              name: file.name.split("/").pop() || "golem.yaml",
              type: "file" as const,
              data: file,
            })),
          })),
        });
      });
    }

    return tree;
  };

  const handleFileSelect = (node: FileTreeNode) => {
    if (node.type === "file" && node.data) {
      // Find the original file ID based on the data
      const file = node.data as YamlFile;
      if (!yamlFiles) return;

      // Check if it's the root file
      if (yamlFiles.root && yamlFiles.root.path === file.path) {
        setSelectedFileId("root");
        return;
      }

      // Check common files
      const commonIndex = yamlFiles.common.findIndex(f => f.path === file.path);
      if (commonIndex !== -1) {
        setSelectedFileId(`common-${commonIndex}`);
        return;
      }

      // Check component files
      const componentIndex = yamlFiles.components.findIndex(
        f => f.path === file.path,
      );
      if (componentIndex !== -1) {
        setSelectedFileId(`component-${componentIndex}`);
      }
    }
  };

  if (isLoading) {
    return (
      <Dialog open={isOpen} onOpenChange={onOpenChange}>
        <DialogContent className="max-w-6xl w-[95vw] h-[85vh] flex flex-col">
          <DialogHeader>
            <DialogTitle>Application Manifests</DialogTitle>
          </DialogHeader>
          <div className="flex items-center justify-center flex-1">
            <div className="text-center">
              <div className="animate-spin rounded-full h-8 w-8 border-b-2 border-primary mx-auto mb-4"></div>
              <p className="text-muted-foreground">Loading YAML files...</p>
            </div>
          </div>
        </DialogContent>
      </Dialog>
    );
  }

  if (!yamlFiles) {
    return (
      <Dialog open={isOpen} onOpenChange={onOpenChange}>
        <DialogContent className="max-w-6xl w-[95vw] h-[85vh] flex flex-col">
          <DialogHeader>
            <DialogTitle>Application Manifests</DialogTitle>
          </DialogHeader>
          <div className="flex items-center justify-center flex-1">
            <div className="text-center">
              <p className="text-muted-foreground">No YAML files found.</p>
            </div>
          </div>
        </DialogContent>
      </Dialog>
    );
  }

  const selectedFile = getSelectedFile();
  return (
    <Dialog open={isOpen} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-6xl w-[95vw] h-[85vh] flex flex-col">
        <DialogHeader className="flex flex-row items-center justify-between mr-6">
          <DialogTitle>Application Manifests</DialogTitle>
          <div className="flex gap-2">
            {selectedFile && editedContents[selectedFile.path] && (
              <Button
                variant="default"
                size="sm"
                onClick={() => handleSave(selectedFile)}
                disabled={savingFiles.has(selectedFile.path)}
              >
                {savingFiles.has(selectedFile.path) ? (
                  <Loader2 className="h-4 w-4 mr-2 animate-spin" />
                ) : (
                  <Save className="h-4 w-4 mr-2" />
                )}
                {savingFiles.has(selectedFile.path) ? "Saving..." : "Save"}
              </Button>
            )}
          </div>
        </DialogHeader>

        <div className="flex gap-4 h-[90%]">
          {/* File Tree Sidebar */}
          <div className="w-64 bg-muted/20 rounded-lg p-3 overflow-y-auto">
            <FileTree
              nodes={buildFileTree()}
              selectedId={selectedFileId}
              onSelect={handleFileSelect}
            />
          </div>

          {/* Editor */}
          <div className="flex-1" style={{ height: "100%" }}>
            {selectedFile ? (
              <YamlEditor
                value={getFileContent(selectedFile)}
                onChange={handleContentChange}
              />
            ) : (
              <div className="h-full flex items-center justify-center text-muted-foreground">
                Select a file to edit
              </div>
            )}
          </div>
        </div>
      </DialogContent>
    </Dialog>
  );
}
