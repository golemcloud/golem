import { useState, useEffect } from "react";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { Alert, AlertDescription } from "@/components/ui/alert";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Loader2, Trash2 } from "lucide-react";
import { profileService } from "@/service/profile";
import { Profile } from "@/types/index";
import { toast } from "@/hooks/use-toast";
import { CreateProfileDialog } from "@/components/create-profile-dialog";

export const ProfileManager = () => {
  const [profiles, setProfiles] = useState<Profile[]>([]);
  const [currentProfile, setCurrentProfile] = useState<Profile | null>(null);
  const [loading, setLoading] = useState(true);
  const [switchingProfile, setSwitchingProfile] = useState<string | null>(null);
  const [deletingProfile, setDeletingProfile] = useState<string | null>(null);
  const [deleteConfirmDialog, setDeleteConfirmDialog] = useState<{
    open: boolean;
    profileName: string;
  }>({ open: false, profileName: "" });

  const loadProfiles = async () => {
    try {
      setLoading(true);
      const [profileList, activeProfile] = await Promise.all([
        profileService.getProfiles(),
        profileService.getCurrentProfile(),
      ]);
      setProfiles(profileList);
      setCurrentProfile(activeProfile);
    } catch (error) {
      toast({
        title: "Error loading profiles",
        description: String(error),
        variant: "destructive",
      });
    } finally {
      setLoading(false);
    }
  };

  const handleSwitchProfile = async (profileName: string) => {
    try {
      setSwitchingProfile(profileName);
      await profileService.switchProfile(profileName);
      await loadProfiles(); // Refresh to get updated active status
      toast({
        title: "Profile switched",
        description: `Successfully switched to ${profileName} profile`,
      });
    } catch (error) {
      toast({
        title: "Error switching profile",
        description: String(error),
        variant: "destructive",
      });
    } finally {
      setSwitchingProfile(null);
    }
  };

  const handleDeleteProfile = async (profileName: string) => {
    // Show confirmation dialog instead of browser confirm
    setDeleteConfirmDialog({
      open: true,
      profileName,
    });
  };

  const confirmDeleteProfile = async () => {
    const profileName = deleteConfirmDialog.profileName;
    setDeleteConfirmDialog({ open: false, profileName: "" });

    try {
      setDeletingProfile(profileName);
      await profileService.deleteProfile(profileName);
      await loadProfiles(); // Refresh to get updated profile list
      toast({
        title: "Profile deleted",
        description: `Successfully deleted ${profileName} profile`,
      });
    } catch (error) {
      toast({
        title: "Error deleting profile",
        description: String(error),
        variant: "destructive",
      });
    } finally {
      setDeletingProfile(null);
    }
  };

  useEffect(() => {
    loadProfiles();
  }, []);

  if (loading) {
    return (
      <div className="flex items-center justify-center p-8">
        <Loader2 className="h-6 w-6 animate-spin" />
        <span className="ml-2">Loading profiles...</span>
      </div>
    );
  }

  return (
    <div className="space-y-4">
      <div className="flex items-center justify-between">
        <h3 className="text-lg font-medium">CLI Profiles</h3>
        <div className="flex space-x-2">
          <CreateProfileDialog onProfileCreated={loadProfiles} />
          <Button onClick={loadProfiles} variant="outline" size="sm">
            Refresh
          </Button>
        </div>
      </div>

      <div className="space-y-3">
        {profiles.map(profile => (
          <div
            key={profile.name}
            className="flex items-center justify-between p-4 border rounded-lg"
          >
            <div className="flex items-center space-x-3">
              <div>
                <div className="flex items-center space-x-2">
                  <span className="font-medium">{profile.name}</span>
                  {profile.is_active && <Badge variant="default">Active</Badge>}
                  <Badge variant="outline" className="text-xs">
                    {profile.kind}
                  </Badge>
                </div>
                <div className="text-sm text-muted-foreground mt-1">
                  {profile.url && <span>URL: {profile.url}</span>}
                </div>
              </div>
            </div>

            <div className="flex items-center space-x-2">
              {!profile.is_active && (
                <Button
                  onClick={() => handleSwitchProfile(profile.name)}
                  disabled={switchingProfile === profile.name}
                  size="sm"
                >
                  {switchingProfile === profile.name ? (
                    <Loader2 className="h-4 w-4 animate-spin" />
                  ) : (
                    "Switch"
                  )}
                </Button>
              )}

              {/* Only show delete button for non-built-in profiles */}
              {!["local", "cloud"].includes(profile.name.toLowerCase()) && (
                <Button
                  onClick={() => handleDeleteProfile(profile.name)}
                  disabled={deletingProfile === profile.name}
                  size="sm"
                  variant="outline"
                  className="text-red-600 hover:text-red-700"
                >
                  {deletingProfile === profile.name ? (
                    <Loader2 className="h-4 w-4 animate-spin" />
                  ) : (
                    <Trash2 className="h-4 w-4" />
                  )}
                </Button>
              )}
            </div>
          </div>
        ))}
      </div>

      {currentProfile && (
        <Alert>
          <AlertDescription>
            Currently using <strong>{currentProfile.name}</strong> profile (
            {currentProfile.kind}) with {currentProfile.config.default_format}{" "}
            output format.
          </AlertDescription>
        </Alert>
      )}

      {/* Delete Confirmation Dialog */}
      <Dialog
        open={deleteConfirmDialog.open}
        onOpenChange={open =>
          !open && setDeleteConfirmDialog({ open: false, profileName: "" })
        }
      >
        <DialogContent className="sm:max-w-md">
          <DialogHeader>
            <DialogTitle>Delete Profile</DialogTitle>
            <DialogDescription>
              Are you sure you want to delete the &quot;
              {deleteConfirmDialog.profileName}&quot; profile? This action
              cannot be undone.
            </DialogDescription>
          </DialogHeader>

          <DialogFooter className="flex-col sm:flex-row gap-2">
            <Button
              variant="outline"
              onClick={() =>
                setDeleteConfirmDialog({ open: false, profileName: "" })
              }
              className="flex-1"
            >
              Cancel
            </Button>
            <Button
              variant="destructive"
              onClick={confirmDeleteProfile}
              className="flex-1"
              disabled={deletingProfile === deleteConfirmDialog.profileName}
            >
              {deletingProfile === deleteConfirmDialog.profileName ? (
                <Loader2 className="h-4 w-4 animate-spin mr-2" />
              ) : null}
              Delete
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
};
