import { useState, useEffect } from "react";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { YamlEditor } from "@/components/yaml-editor";
import { Save } from "lucide-react";
import { toast } from "@/hooks/use-toast";
import { API } from "@/service";

interface YamlViewerModalProps {
  isOpen: boolean;
  onOpenChange: (open: boolean) => void;
  title: string;
  yamlContent: string;
  onSave?: (content: string) => Promise<void>;
  // For saving to app or component
  appId?: string;
  componentId?: string;
  isAppYaml?: boolean;
}

export function YamlViewerModal({
  isOpen,
  onOpenChange,
  title,
  yamlContent: initialContent,
  onSave,
  appId,
  componentId,
  isAppYaml = false,
}: YamlViewerModalProps) {
  const [content, setContent] = useState(initialContent);
  const [isSaving, setIsSaving] = useState(false);

  const handleSave = async () => {
    setIsSaving(true);
    try {
      if (onSave) {
        await onSave(content);
      } else if (appId) {
        // Use built-in save functionality
        if (isAppYaml) {
          await API.manifestService.saveAppManifest(appId, content);
          toast({
            title: "App YAML Saved",
            description:
              "The application manifest has been saved successfully.",
          });
        } else if (componentId) {
          await API.manifestService.saveComponentManifest(
            appId,
            componentId,
            content,
          );
          toast({
            title: "Component YAML Saved",
            description: "The component manifest has been saved successfully.",
          });
        }
      }
    } catch (error) {
      toast({
        title: "Save Failed",
        description: String(error),
        variant: "destructive",
      });
    } finally {
      setIsSaving(false);
    }
  };

  // Update content when initialContent changes
  useEffect(() => {
    setContent(initialContent);
  }, [initialContent]);

  return (
    <Dialog open={isOpen} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-4xl w-[90vw] h-[80vh] flex flex-col">
        <DialogHeader className="flex flex-row items-center justify-between mr-6">
          <DialogTitle>{title}</DialogTitle>
          <div className="flex gap-2">
            {(onSave || (appId && (isAppYaml || componentId))) && (
              <Button
                variant="default"
                size="sm"
                onClick={handleSave}
                disabled={isSaving}
              >
                <Save className="h-4 w-4" />
                {isSaving ? "Saving..." : "Save"}
              </Button>
            )}
          </div>
        </DialogHeader>

        <div className="flex-1 mt-4">
          <YamlEditor value={content} onChange={setContent} />
        </div>

        <div className="mt-4 text-sm text-muted-foreground">
          <p>
            Edit the YAML content above. Use the save button to persist changes.
          </p>
        </div>
      </DialogContent>
    </Dialog>
  );
}
