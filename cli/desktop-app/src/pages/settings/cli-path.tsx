import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { GolemCliPathSetting } from "@/components/golem-cli-path";

export const CliPathSettingsPage = () => {
  return (
    <div className="space-y-6">
      <div>
        <h2 className="text-2xl font-bold">CLI Path Configuration</h2>
        <p className="text-muted-foreground">
          Configure the path to the golem-cli executable on your system.
        </p>
      </div>

      <Card>
        <CardHeader>
          <CardTitle>Golem CLI Path</CardTitle>
          <CardDescription>
            Specify the path to the golem-cli executable. If not set, the
            application will try to find golem-cli in your system PATH.
          </CardDescription>
        </CardHeader>
        <CardContent>
          <GolemCliPathSetting />
        </CardContent>
      </Card>
    </div>
  );
};
