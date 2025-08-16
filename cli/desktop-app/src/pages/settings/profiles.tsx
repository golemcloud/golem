import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { ProfileManager } from "@/components/profile-manager";

export const ProfileSettingsPage = () => {
  return (
    <div className="space-y-6">
      <div>
        <h2 className="text-2xl font-bold">CLI Profiles</h2>
        <p className="text-muted-foreground">
          Manage and switch between different Golem CLI profiles (local, cloud,
          custom).
        </p>
      </div>

      <Card>
        <CardHeader>
          <CardTitle>Profile Management</CardTitle>
          <CardDescription>
            Create, switch, and manage your Golem CLI profiles. Profiles allow
            you to easily switch between different Golem environments.
          </CardDescription>
        </CardHeader>
        <CardContent>
          <ProfileManager />
        </CardContent>
      </Card>
    </div>
  );
};
