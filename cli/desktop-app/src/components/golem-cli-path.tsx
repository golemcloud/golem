import { useState, useEffect } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { FolderOpen, Save, Check } from "lucide-react";
import { open } from "@tauri-apps/plugin-dialog";
import { toast } from "@/hooks/use-toast";
import { settingsService } from "@/lib/settings";

export function GolemCliPathSetting() {
  const [golemCliPath, setGolemCliPath] = useState("");
  const [isSaving, setIsSaving] = useState(false);
  const [hasSaved, setHasSaved] = useState(false);

  useEffect(() => {
    // Load the current golem-cli path on component mount
    loadGolemCliPath();
  }, []);

  const loadGolemCliPath = async () => {
    try {
      const path = await settingsService.getGolemCliPath();
      if (path) {
        setGolemCliPath(path);
        setHasSaved(true); // If we have a saved path, mark as saved
      }
    } catch (error) {
      console.error("Error loading golem-cli path:", error);
    }
  };

  const handleBrowse = async () => {
    try {
      // Open a dialog to select the golem-cli executable
      const selected = await open({
        multiple: false,
        title: "Select golem-cli executable",
        filters: [
          {
            name: "golem-cli",
            extensions: [],
          },
        ],
      });

      if (selected && typeof selected === "string") {
        setGolemCliPath(selected);
        setHasSaved(false);
      }
    } catch (error) {
      console.error("Error selecting golem-cli path:", error);
      toast({
        title: "Error selecting golem-cli path",
        description: String(error),
        variant: "destructive",
      });
    }
  };

  const handleSave = async () => {
    if (!golemCliPath) {
      toast({
        title: "Please select a path",
        variant: "destructive",
      });
      return;
    }

    setIsSaving(true);
    try {
      const success = await settingsService.setGolemCliPath(golemCliPath);

      if (success) {
        toast({
          title: "golem-cli path saved",
          description: "The path has been saved successfully.",
        });
        setHasSaved(true);
      } else {
        throw new Error("Failed to save path");
      }
    } catch (error) {
      console.error("Error saving golem-cli path:", error);
      toast({
        title: "Error saving golem-cli path",
        description: String(error),
        variant: "destructive",
      });
    } finally {
      setIsSaving(false);
    }
  };

  return (
    <div className="space-y-4">
      <div className="flex flex-col space-y-1.5">
        <Label htmlFor="golem-cli-path">golem-cli Path</Label>
        <div className="flex gap-2">
          <Input
            id="golem-cli-path"
            value={golemCliPath}
            onChange={e => {
              setGolemCliPath(e.target.value);
              setHasSaved(false);
            }}
            placeholder="Select golem-cli executable path"
            className="flex-1"
            // disable edit
            readOnly
          />
          <Button variant="outline" onClick={handleBrowse} type="button">
            <FolderOpen size={16} className="mr-2" />
            Browse
          </Button>
          <Button
            onClick={handleSave}
            disabled={isSaving || hasSaved}
            type="button"
          >
            {isSaving ? (
              "Saving..."
            ) : hasSaved ? (
              <>
                <Check size={16} className="mr-2" />
                Saved
              </>
            ) : (
              <>
                <Save size={16} className="mr-2" />
                Save
              </>
            )}
          </Button>
        </div>
        <p className="text-sm text-muted-foreground">
          Specify the path to the golem-cli executable. If not set, the system
          will use golem-cli from your PATH.
        </p>
      </div>
    </div>
  );
}
