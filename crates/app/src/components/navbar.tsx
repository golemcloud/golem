import {ModeToggle} from "@/components/mode-toggle.tsx";
import NavLink from "@/components/navLink.tsx";
import {Logo} from "@/components/logo.tsx";

const Navbar = () => {
    return (
        <nav className="border-b">
            <div className="flex items-center justify-between px-4 py-2">
                <div className="flex items-center space-x-8">
                    <div className="flex items-center space-x-2">
                        <a href="/">
                            <Logo/>
                        </a>
                    </div>
                    <div className="flex space-x-6">
                        <NavLink to="/">Dashboard</NavLink>
                        <NavLink to="/components">Components</NavLink>
                        <NavLink to="/apis">APIs</NavLink>
                        <NavLink to="/deployments">Deployments</NavLink>
                        <NavLink to="/plugins">Plugins</NavLink>
                    </div>
                </div>
                <div className="flex items-center space-x-8">
                    <div className="flex items-center space-x-2">
                        <ModeToggle/>
                    </div>
                </div>
            </div>
        </nav>
    );
};

export default Navbar;
