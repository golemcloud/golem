import { useState, useEffect } from "react";
import { useNavigate } from "react-router-dom";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import {
  Folder,
  FolderOpen,
  Plus,
  ChevronRight,
  Clock,
  ArrowRight,
} from "lucide-react";
import { open } from "@tauri-apps/plugin-dialog";
import { toast } from "@/hooks/use-toast";
import { settingsService, App } from "@/lib/settings";
// Helper function to format relative time (e.g., "2 days ago")
const formatRelativeTime = (date: Date): string => {
  const now = new Date();
  const diffInSeconds = Math.floor((now.getTime() - date.getTime()) / 1000);

  if (diffInSeconds < 60) return "just now";

  const diffInMinutes = Math.floor(diffInSeconds / 60);
  if (diffInMinutes < 60)
    return `${diffInMinutes} minute${diffInMinutes > 1 ? "s" : ""} ago`;

  const diffInHours = Math.floor(diffInMinutes / 60);
  if (diffInHours < 24)
    return `${diffInHours} hour${diffInHours > 1 ? "s" : ""} ago`;

  const diffInDays = Math.floor(diffInHours / 24);
  if (diffInDays < 30)
    return `${diffInDays} day${diffInDays > 1 ? "s" : ""} ago`;

  const diffInMonths = Math.floor(diffInDays / 30);
  if (diffInMonths < 12)
    return `${diffInMonths} month${diffInMonths > 1 ? "s" : ""} ago`;

  const diffInYears = Math.floor(diffInMonths / 12);
  return `${diffInYears} year${diffInYears > 1 ? "s" : ""} ago`;
};

// Using the App interface from settingsService

export const Home = () => {
  const navigate = useNavigate();
  const [isOpeningApp, setIsOpeningApp] = useState(false);
  const [searchTerm, setSearchTerm] = useState("");
  const [recentApps, setRecentApps] = useState<App[]>([]);
  const [showAllApps, setShowAllApps] = useState(false);

  // Fetch apps from settings service
  useEffect(() => {
    const fetchApps = async () => {
      try {
        const apps = await settingsService.getApps();
        setRecentApps(apps || []);
      } catch (error) {
        console.error("Failed to fetch apps:", error);
      }
    };

    fetchApps();
  }, []);

  const handleCreateApp = () => {
    // Navigate to the app creation page
    navigate("/app-create");
  };

  const handleOpenApp = async () => {
    setIsOpeningApp(true);
    try {
      // Open a dialog to select the app folder
      const selected = await open({
        directory: true,
        multiple: false,
        title: "Select Golem Application Folder",
      });

      if (selected) {
        // Validate the selected folder contains golem.yaml
        const validation = await settingsService.validateGolemApp(selected);

        if (validation.isValid) {
          // Create a unique ID for the app
          const appId = `app-${Date.now()}`;

          // Create app object
          const app: App = {
            id: appId,
            folderLocation: selected,
            golemYamlLocation: validation.yamlPath,
            lastOpened: new Date().toISOString(),
          };

          // Save to store
          const saved = await settingsService.addApp(app);

          if (saved) {
            // Update the recentApps state to show the new app
            setRecentApps(await settingsService.getApps());
            // Navigate to the app
            navigate(`/app/${appId}`);
          }
        } else {
          toast({
            title: "Invalid Golem Application",
            description:
              "The selected folder does not contain a golem.yaml file.",
            variant: "destructive",
          });
        }
      }
    } catch (error) {
      console.error("Failed to open app:", error);
      toast({
        title: "Error opening application",
        description: String(error),
        variant: "destructive",
      });
    } finally {
      setIsOpeningApp(false);
    }
  };

  // Filter recent apps based on search term
  const filteredApps = recentApps
    .filter(
      app =>
        (app.name?.toLowerCase() || "").includes(searchTerm.toLowerCase()) ||
        app.folderLocation.toLowerCase().includes(searchTerm.toLowerCase()),
    ) // sort by lastOpened date, most recent first
    .sort((a, b) => {
      const dateA = new Date(a.lastOpened || 0);
      const dateB = new Date(b.lastOpened || 0);
      return dateB.getTime() - dateA.getTime();
    });

  // Display only recent apps unless showAllApps is true
  const appsToShow = showAllApps ? filteredApps : filteredApps.slice(0, 6);

  return (
    <div className="container mx-auto px-4 py-8">
      <div className="flex flex-col space-y-8">
        <div className="flex items-center justify-between">
          <div className="flex items-center gap-2">
            {/* <Logo /> */}
            <h1 className="text-3xl font-bold">Golem Desktop</h1>
          </div>
          <Button
            variant="outline"
            className="flex items-center gap-2"
            onClick={() => navigate("/app-create")}
          >
            <Plus size={16} />
            New Application
          </Button>
        </div>

        {/* Action cards - horizontal layout */}
        <div className="grid grid-cols-1 md:grid-cols-2 gap-6">
          {/* Create App Card */}
          <Card className="p-6 h-full flex flex-col">
            <CardHeader className="pb-2">
              <CardTitle className="text-2xl">Create New Application</CardTitle>
              <CardDescription>
                Start a new Golem application project
              </CardDescription>
            </CardHeader>
            <CardContent className="flex-1 flex items-center justify-center">
              <Button
                size="lg"
                className="w-full flex items-center justify-center gap-2 py-8"
                onClick={handleCreateApp}
              >
                <Plus size={24} />
                <span className="text-lg">Create New Application</span>
              </Button>
            </CardContent>
          </Card>

          {/* Open App Card */}
          <Card className="p-6 h-full flex flex-col">
            <CardHeader className="pb-2">
              <CardTitle className="text-2xl">
                Open Existing Application
              </CardTitle>
              <CardDescription>
                Open and work with an existing Golem application
              </CardDescription>
            </CardHeader>
            <CardContent className="flex-1 flex items-center justify-center">
              <Button
                size="lg"
                variant="outline"
                className="w-full flex items-center justify-center gap-2 py-8"
                onClick={handleOpenApp}
                disabled={isOpeningApp}
              >
                <FolderOpen size={24} />
                <span className="text-lg">
                  {isOpeningApp ? "Opening..." : "Open"}
                </span>
              </Button>
            </CardContent>
          </Card>
        </div>

        {/* Recent apps section - full width */}
        <Card className="w-full">
          <CardHeader className="pb-2">
            <div className="flex justify-between items-center">
              <div>
                <CardTitle>Recent Applications</CardTitle>
                <CardDescription>
                  Your recently opened applications
                </CardDescription>
              </div>
              {recentApps.length > 3 && (
                <Button
                  variant="ghost"
                  size="sm"
                  className="text-sm"
                  onClick={() => setShowAllApps(!showAllApps)}
                >
                  {showAllApps ? "Show Less" : "View All"}
                  <ChevronRight size={16} className="ml-1" />
                </Button>
              )}
            </div>
          </CardHeader>
          <CardContent>
            {recentApps.length === 0 ? (
              <div className="text-center py-8 text-muted-foreground">
                <p>No recent applications found</p>
              </div>
            ) : (
              <div className="space-y-2">
                <Input
                  placeholder="Search applications..."
                  value={searchTerm}
                  onChange={e => setSearchTerm(e.target.value)}
                  className="mb-4"
                />

                {appsToShow.length === 0 ? (
                  <div className="text-center py-4 text-muted-foreground">
                    <p>No matching applications found</p>
                  </div>
                ) : (
                  <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-4 max-h-[500px] overflow-y-auto pr-2">
                    {appsToShow.map((app, index) => (
                      <Card
                        key={app.id || index}
                        className="cursor-pointer hover:bg-muted/50 transition-all border-l-4 border-l-primary/70"
                        onClick={() => navigate(`/app/${app.id}`)}
                      >
                        <CardContent className="p-4">
                          <div className="flex justify-between items-center">
                            <div>
                              <h3 className="font-medium text-base">
                                {app.name}
                              </h3>
                              <p className="text-sm text-muted-foreground flex items-center gap-1 mt-1">
                                <Folder size={14} />
                                {app.folderLocation.length > 34
                                  ? "..." + app.folderLocation.slice(-(34 - 3))
                                  : app.folderLocation}
                              </p>
                              {app.lastOpened && (
                                <div className="flex items-center gap-1 mt-2 text-xs text-muted-foreground">
                                  <Clock size={12} />
                                  Last opened:{" "}
                                  {formatRelativeTime(new Date(app.lastOpened))}
                                </div>
                              )}
                            </div>
                            <Button
                              size="sm"
                              variant="ghost"
                              className="rounded-full h-8 w-8 p-0"
                            >
                              <ArrowRight size={16} />
                            </Button>
                          </div>
                        </CardContent>
                      </Card>
                    ))}
                  </div>
                )}
              </div>
            )}
          </CardContent>
        </Card>
      </div>
    </div>
  );
};

export default Home;
