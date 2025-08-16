import { useLocation, useNavigate } from "react-router-dom";
import { cn } from "@/lib/utils";
import { Button } from "@/components/ui/button";
import { User, Terminal } from "lucide-react";
import { Outlet } from "react-router-dom";

const settingsNavigation = [
  {
    name: "CLI Profiles",
    href: "/settings",
    icon: User,
    description: "Manage Golem CLI profiles",
  },
  {
    name: "CLI Path",
    href: "/settings/cli-path",
    icon: Terminal,
    description: "Configure Golem CLI executable path",
  },
];

export const SettingsLayout = () => {
  const location = useLocation();
  const navigate = useNavigate();

  return (
    <div className="min-h-screen bg-background">
      <div className="container mx-auto px-4 py-8">
        <div className="flex flex-col space-y-8 max-w-7xl mx-auto">
          <h1 className="text-3xl font-bold">Settings</h1>

          <div className="flex gap-8">
            {/* Sidebar */}
            <div className="w-72 flex-shrink-0">
              <div className="sticky top-4">
                <nav className="space-y-2">
                  {settingsNavigation.map(item => {
                    const Icon = item.icon;
                    const isActive = location.pathname === item.href;

                    return (
                      <Button
                        key={item.name}
                        variant={isActive ? "secondary" : "ghost"}
                        className={cn(
                          "w-full justify-start text-left h-auto p-4 whitespace-normal",
                          isActive && "bg-secondary",
                        )}
                        onClick={() => navigate(item.href)}
                      >
                        <div className="flex items-start space-x-3 w-full">
                          <Icon className="h-5 w-5 mt-0.5 flex-shrink-0" />
                          <div className="flex-1 min-w-0 text-left">
                            <div className="font-medium">{item.name}</div>
                            <div className="text-sm text-muted-foreground mt-1 leading-snug">
                              {item.description}
                            </div>
                          </div>
                        </div>
                      </Button>
                    );
                  })}
                </nav>
              </div>
            </div>

            {/* Main content */}
            <div className="flex-1 min-w-0 max-w-4xl">
              <Outlet />
            </div>
          </div>
        </div>
      </div>
    </div>
  );
};
