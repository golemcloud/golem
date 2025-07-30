import { Logo } from "@/components/logo.tsx";
import { ModeToggle } from "@/components/mode-toggle.tsx";
import NavLink from "@/components/navLink.tsx";
import { ServerStatus } from "./server-status";
import { useParams } from "react-router-dom";
import { Settings } from "lucide-react";
import { Button } from "./ui/button";

interface NavbarProps {
  showNav?: boolean;
}

const Navbar = ({ showNav = true }: NavbarProps) => {
  const { appId } = useParams();

  return (
    <nav className="border-b">
      <div className="flex items-center justify-between px-4 py-2">
        <div className="flex items-center space-x-8">
          <div className="flex items-center space-x-2">
            <a href="/">
              <Logo />
            </a>
          </div>
          {showNav && appId && (
            <div className="flex space-x-6">
              <NavLink to={`/app/${appId}/dashboard`}>Dashboard</NavLink>
              <NavLink to={`/app/${appId}/components`}>Components</NavLink>
              <NavLink to={`/app/${appId}/apis`}>APIs</NavLink>
              <NavLink to={`/app/${appId}/deployments`}>Deployments</NavLink>
              <NavLink to={`/app/${appId}/plugins`}>Plugins</NavLink>
            </div>
          )}
        </div>
        <div className="flex items-center space-x-8">
          <div className="flex items-center space-x-2">
            <ServerStatus />
            <ModeToggle />
            <NavLink to="/settings">
              <Button variant="outline" size="icon">
                <Settings className="h-4 w-4" />
              </Button>
            </NavLink>
          </div>
        </div>
      </div>
    </nav>
  );
};

export default Navbar;
