import NavLink from "./NavLink";
import logo from "/logo.svg";
import {ModeToggle} from "@/components/mode-toggle.tsx";

const Navbar = () => {
  return (
    <nav className="border-b">
      <div className="flex items-center justify-between px-4 py-2">
        <div className="flex items-center space-x-8">
          <div className="flex items-center space-x-2">
            <a href="/">
              <img className="logo-light fill-foreground stroke-foreground h-8 w-8 overflow-visible transition-opacity hover:opacity-80"
                   src={logo} alt="Golem Logo"/>
            </a>
          </div>
          <div className="flex space-x-6">
            <NavLink to="/">Overview</NavLink>
            <NavLink to="/components">Components</NavLink>
            <NavLink to="/apis">APIs</NavLink>
            <NavLink to="/workers">Workers</NavLink>
            <NavLink to="/deployments">Deployments</NavLink>
            <NavLink to="/plugins">Plugins</NavLink>
          </div>
        </div>
        <div className="flex items-center space-x-8">
          <div className="flex items-center space-x-2">
            <ModeToggle />
          </div>
        </div>
      </div>
    </nav>
  );
};

export default Navbar;
