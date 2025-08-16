import { useState } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Checkbox } from "@/components/ui/checkbox";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from "@/components/ui/dialog";
import { Loader2, Plus } from "lucide-react";
import { profileService } from "@/service/profile";
import { toast } from "@/hooks/use-toast";

interface CreateProfileDialogProps {
  onProfileCreated: () => void;
}

export const CreateProfileDialog = ({
  onProfileCreated,
}: CreateProfileDialogProps) => {
  const [open, setOpen] = useState(false);
  const [loading, setLoading] = useState(false);
  const [formData, setFormData] = useState({
    name: "",
    kind: "oss" as "oss" | "cloud",
    setActive: false,
    componentUrl: "",
    workerUrl: "",
    cloudUrl: "",
    defaultFormat: "text",
  });

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();

    if (!formData.name.trim()) {
      toast({
        title: "Validation Error",
        description: "Profile name is required",
        variant: "destructive",
      });
      return;
    }

    try {
      setLoading(true);

      const options: {
        setActive?: boolean;
        componentUrl?: string;
        workerUrl?: string;
        cloudUrl?: string;
        defaultFormat?: string;
      } = {
        setActive: formData.setActive,
        defaultFormat: formData.defaultFormat,
      };

      if (formData.componentUrl) {
        options.componentUrl = formData.componentUrl;
      }
      if (formData.workerUrl) {
        options.workerUrl = formData.workerUrl;
      }
      if (formData.cloudUrl) {
        options.cloudUrl = formData.cloudUrl;
      }

      await profileService.createProfile(
        formData.kind === "cloud" ? "Cloud" : "Oss",
        formData.name,
        options,
      );

      toast({
        title: "Profile Created",
        description: `Successfully created ${formData.name} profile`,
      });

      // Reset form
      setFormData({
        name: "",
        kind: "oss",
        setActive: false,
        componentUrl: "",
        workerUrl: "",
        cloudUrl: "",
        defaultFormat: "text",
      });

      setOpen(false);
      onProfileCreated();
    } catch (error) {
      toast({
        title: "Error creating profile",
        description: String(error),
        variant: "destructive",
      });
    } finally {
      setLoading(false);
    }
  };

  const resetForm = () => {
    setFormData({
      name: "",
      kind: "oss",
      setActive: false,
      componentUrl: "",
      workerUrl: "",
      cloudUrl: "",
      defaultFormat: "text",
    });
  };

  return (
    <Dialog
      open={open}
      onOpenChange={newOpen => {
        setOpen(newOpen);
        if (!newOpen) {
          resetForm();
        }
      }}
    >
      <DialogTrigger asChild>
        <Button size="sm">
          <Plus className="h-4 w-4 mr-1" />
          New Profile
        </Button>
      </DialogTrigger>

      <DialogContent className="sm:max-w-[500px]">
        <DialogHeader>
          <DialogTitle>Create New Profile</DialogTitle>
        </DialogHeader>

        <form onSubmit={handleSubmit} className="space-y-4">
          <div className="grid grid-cols-2 gap-4">
            <div className="space-y-2">
              <Label htmlFor="name">Profile Name</Label>
              <Input
                id="name"
                value={formData.name}
                onChange={e =>
                  setFormData(prev => ({ ...prev, name: e.target.value }))
                }
                placeholder="my-profile"
                required
              />
            </div>

            <div className="space-y-2">
              <Label htmlFor="kind">Profile Type</Label>
              <Select
                value={formData.kind}
                onValueChange={(value: "oss" | "cloud") =>
                  setFormData(prev => ({ ...prev, kind: value }))
                }
              >
                <SelectTrigger>
                  <SelectValue placeholder="Select type" />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="oss">Local/OSS</SelectItem>
                  <SelectItem value="cloud">Cloud</SelectItem>
                </SelectContent>
              </Select>
            </div>
          </div>

          <div className="space-y-2">
            <Label htmlFor="defaultFormat">Default Output Format</Label>
            <Select
              value={formData.defaultFormat}
              onValueChange={value =>
                setFormData(prev => ({ ...prev, defaultFormat: value }))
              }
            >
              <SelectTrigger>
                <SelectValue placeholder="Select format" />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="text">Text</SelectItem>
                <SelectItem value="json">JSON</SelectItem>
                <SelectItem value="yaml">YAML</SelectItem>
              </SelectContent>
            </Select>
          </div>

          {formData.kind === "oss" && (
            <div className="space-y-4">
              <div className="space-y-2">
                <Label htmlFor="componentUrl">Component Service URL</Label>
                <Input
                  id="componentUrl"
                  value={formData.componentUrl}
                  onChange={e =>
                    setFormData(prev => ({
                      ...prev,
                      componentUrl: e.target.value,
                    }))
                  }
                  placeholder="http://localhost:9881"
                />
              </div>

              <div className="space-y-2">
                <Label htmlFor="workerUrl">Worker Service URL (optional)</Label>
                <Input
                  id="workerUrl"
                  value={formData.workerUrl}
                  onChange={e =>
                    setFormData(prev => ({
                      ...prev,
                      workerUrl: e.target.value,
                    }))
                  }
                  placeholder="Defaults to component URL"
                />
              </div>
            </div>
          )}

          {formData.kind === "cloud" && (
            <div className="space-y-2">
              <Label htmlFor="cloudUrl">Cloud Service URL (optional)</Label>
              <Input
                id="cloudUrl"
                value={formData.cloudUrl}
                onChange={e =>
                  setFormData(prev => ({ ...prev, cloudUrl: e.target.value }))
                }
                placeholder="Defaults to standard cloud URL"
              />
            </div>
          )}

          <div className="flex items-center space-x-2">
            <Checkbox
              id="setActive"
              checked={formData.setActive}
              onCheckedChange={(checked: boolean) =>
                setFormData(prev => ({ ...prev, setActive: checked }))
              }
            />
            <Label htmlFor="setActive">
              Set as active profile after creation
            </Label>
          </div>

          <div className="flex justify-end space-x-2 pt-4">
            <Button
              type="button"
              variant="outline"
              onClick={() => setOpen(false)}
              disabled={loading}
            >
              Cancel
            </Button>
            <Button type="submit" disabled={loading}>
              {loading && <Loader2 className="h-4 w-4 mr-2 animate-spin" />}
              Create Profile
            </Button>
          </div>
        </form>
      </DialogContent>
    </Dialog>
  );
};
