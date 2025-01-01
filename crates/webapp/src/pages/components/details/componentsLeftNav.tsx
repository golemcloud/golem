import React from "react";
import { useNavigate, useParams, useLocation } from "react-router-dom";
import { Home, Settings, ArrowRightFromLine, Pencil } from "lucide-react";

const ComponentLeftNav = () => {
  const navigate = useNavigate();
  const { componentId } = useParams();
  const location = useLocation();

  const isActive = (path: string) => location.pathname.endsWith(path);

  return (
    <nav className="w-64 border-r border-gray-200  p-4">
      <div className="mb-8">
        <ul className="space-y-2">
          <li>
            <button
              onClick={() => navigate(`/components/${componentId}`)}
              className={`w-full text-left px-3 py-2 rounded-md ${
                isActive(componentId!) ? "bg-gray-200" : "hover:bg-gray-100"
              }`}
            >
              <div className="flex items-center">
                <Home className="h-4 w-4 mr-2" />
                <span>Overview</span>
              </div>
            </button>
          </li>
          <li>
            <button
              onClick={() => navigate(`/components/${componentId}/exports`)}
              className={`w-full text-left px-3 py-2 rounded-md ${
                isActive("exports") ? "bg-gray-200" : "hover:bg-gray-100"
              }`}
            >
              <div className="flex items-center">
                <ArrowRightFromLine className="h-4 w-4 mr-2" />
                <span>Exports</span>
              </div>
            </button>
          </li>
          <li>
            <button
              onClick={() => navigate(`/components/${componentId}/settings`)}
              className={`w-full text-left px-3 py-2 rounded-md ${
                isActive("settings") ? "bg-gray-200" : "hover:bg-gray-100"
              }`}
            >
              <div className="flex items-center">
                <Settings className="h-4 w-4 mr-2" />
                <span>Settings</span>
              </div>
            </button>
          </li>
          <li>
            <button
              onClick={() => navigate(`/components/${componentId}/update`)}
              className={`w-full text-left px-3 py-2 rounded-md ${
                isActive("update") ? "bg-gray-200" : "hover:bg-gray-100"
              }`}
            >
              <div className="flex items-center">
                <Pencil className="h-4 w-4 mr-2" />
                <span>Update</span>
              </div>
            </button>
          </li>
        </ul>
      </div>
    </nav>
  );
};

export default ComponentLeftNav;
