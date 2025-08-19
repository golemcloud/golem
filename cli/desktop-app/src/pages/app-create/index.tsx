import { useState } from "react";
import { useNavigate } from "react-router-dom";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { toast } from "@/hooks/use-toast";
import { settingsService } from "@/lib/settings";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { FolderOpen, Info, ArrowLeft, Sparkles } from "lucide-react";

const LANGUAGE_OPTIONS = [
  { value: "c", label: "C" },
  { value: "go", label: "Go" },
  { value: "js", label: "JavaScript" },
  { value: "python", label: "Python" },
  { value: "rust", label: "Rust" },
  { value: "ts", label: "TypeScript" },
  { value: "zig", label: "Zig" },
  { value: "scala", label: "Scala.js" },
  { value: "moonbit", label: "MoonBit" },
];

export const CreateApplication = () => {
  const navigate = useNavigate();
  const [isCreating, setIsCreating] = useState(false);
  const [formData, setFormData] = useState({
    appName: "",
    language: "",
    folderPath: "",
  });

  const [folderError, setFolderError] = useState("");
  const [nameError, setNameError] = useState("");

  // Form validation
  const isFormValid = () => {
    let isValid = true;

    if (!formData.appName.trim()) {
      setNameError("Application name is required");
      isValid = false;
    } else if (!/^[a-zA-Z0-9_-]+$/.test(formData.appName)) {
      setNameError(
        "Application name can only contain alphanumeric characters, hyphens, and underscores",
      );
      isValid = false;
    } else {
      setNameError("");
    }

    if (!formData.folderPath) {
      setFolderError("Root folder is required");
      isValid = false;
    } else {
      setFolderError("");
    }

    if (!formData.language) {
      toast({
        title: "Please select a programming language",
        variant: "destructive",
      });
      isValid = false;
    }

    return isValid;
  };

  // Handle directory selection
  const handleSelectFolder = async () => {
    try {
      // Open a dialog to select a directory
      const selected = await open({
        directory: true,
        multiple: false,
        title: "Select root folder",
      });

      if (selected && typeof selected === "string") {
        setFormData({
          ...formData,
          folderPath: selected,
        });
        setFolderError("");
      }
    } catch (error) {
      console.error("Error selecting folder:", error);
      toast({
        title: "Error selecting folder",
        description: String(error),
        variant: "destructive",
      });
    }
  };

  // Handle form submission
  const handleSubmit = async () => {
    if (!isFormValid()) return;

    setIsCreating(true);
    try {
      // Call the Rust function to create the application
      const result = await invoke("create_golem_app", {
        folderPath: formData.folderPath,
        appName: formData.appName,
        language: formData.language,
      });

      // Create app object to save to store
      const appPath = `${formData.folderPath}/${formData.appName}`;
      const appId = `app-${Date.now()}`;

      // Save the new application to store
      await settingsService.addApp({
        id: appId,
        folderLocation: appPath,
        golemYamlLocation: `${appPath}/golem.yaml`,
        lastOpened: new Date().toISOString(),
      });

      toast({
        title: "Application created successfully",
        description: String(result),
      });

      // Navigate to home page after successful creation
      navigate("/");
    } catch (error) {
      console.error("Error creating application:", error);
      toast({
        title: "Error creating application",
        description: String(error),
        variant: "destructive",
      });
    } finally {
      setIsCreating(false);
    }
  };

  return (
    <div className="container mx-auto px-4 py-8">
      <div className="flex flex-col space-y-8 max-w-2xl mx-auto">
        <div className="flex items-center">
          <Button
            variant="ghost"
            onClick={() => navigate("/")}
            className="mr-4"
          >
            <ArrowLeft size={16} className="mr-2" />
            Back
          </Button>
          <h1 className="text-3xl font-bold">Create New Application</h1>
        </div>

        <Card>
          <CardHeader>
            <CardTitle>Application Details</CardTitle>
            <CardDescription>
              Enter the details for your new Golem application
            </CardDescription>
          </CardHeader>
          <CardContent className="space-y-6">
            {/* Application Name */}
            <div className="space-y-2">
              <Label htmlFor="app-name">Application Name</Label>
              <Input
                id="app-name"
                placeholder="my-golem-app"
                value={formData.appName}
                onChange={e => {
                  setFormData({ ...formData, appName: e.target.value });
                  if (e.target.value) setNameError("");
                }}
                className={nameError ? "border-red-500" : ""}
              />
              {nameError && <p className="text-red-500 text-sm">{nameError}</p>}
            </div>

            {/* Language Selection */}
            <div className="space-y-2">
              <Label htmlFor="language">Programming Language</Label>
              <Select
                value={formData.language}
                onValueChange={value =>
                  setFormData({ ...formData, language: value })
                }
              >
                <SelectTrigger>
                  <SelectValue placeholder="Select a language" />
                </SelectTrigger>
                <SelectContent>
                  {LANGUAGE_OPTIONS.map(option => (
                    <SelectItem key={option.value} value={option.value}>
                      {option.label}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>

            {/* Root Folder Selection */}
            <div className="space-y-2">
              <div className="flex items-center gap-2">
                <Label htmlFor="folder-path">Root Folder</Label>
                <TooltipProvider>
                  <Tooltip>
                    <TooltipTrigger asChild>
                      <div className="cursor-help">
                        <Info size={14} className="text-muted-foreground" />
                      </div>
                    </TooltipTrigger>
                    <TooltipContent>
                      <p>
                        Your application will be created in:{" "}
                        {formData.folderPath}/{formData.appName}
                      </p>
                    </TooltipContent>
                  </Tooltip>
                </TooltipProvider>
              </div>
              <div className="flex gap-2">
                <Input
                  id="folder-path"
                  placeholder="Select a folder"
                  value={formData.folderPath}
                  onChange={e =>
                    setFormData({ ...formData, folderPath: e.target.value })
                  }
                  className={`flex-1 ${folderError ? "border-red-500" : ""}`}
                  readOnly
                  // foward click to the button
                  onClick={handleSelectFolder}
                />
                <Button
                  variant="outline"
                  onClick={handleSelectFolder}
                  type="button"
                >
                  <FolderOpen size={16} className="mr-2" />
                  Browse
                </Button>
              </div>
              {folderError && (
                <p className="text-red-500 text-sm">{folderError}</p>
              )}
              {formData.folderPath && formData.appName && (
                <p className="text-sm text-muted-foreground">
                  Project will be created at:{" "}
                  <span className="font-mono">
                    {formData.folderPath}/{formData.appName}
                  </span>
                </p>
              )}
            </div>

            {/* Create Button */}
            <Button
              className="w-full mt-6"
              onClick={handleSubmit}
              disabled={isCreating}
            >
              <Sparkles size={16} className="mr-2" />
              {isCreating ? "Creating Application..." : "Create Application"}
            </Button>
          </CardContent>
        </Card>
      </div>
    </div>
  );
};

export default CreateApplication;
