import { useNavigate, useParams, useLocation } from "react-router-dom";
import {
  Home,
  Settings,
  Plus,
  ArrowLeft,
  CircleFadingPlusIcon,
} from "lucide-react";

const APILeftNav = () => {
  const navigate = useNavigate();
  const { apiName } = useParams();
  const location = useLocation();

  const isActive = (path: string) => location.pathname.endsWith(path);

  return (
    <nav className="w-64 border-r border-gray-200 p-4">
      <div className="mb-8">
        <button
          onClick={() => navigate(`/apis`)}
          className="text-xl  flex items-center text-gray-800 hover:text-gray-900 mb-4"
        >
          <ArrowLeft className="h-4 w-4 mr-2" />
          <span>API</span>
        </button>
        <ul className="space-y-2">
          <li>
            <button
              onClick={() => navigate(`/apis/${apiName}`)}
              className={`w-full text-left px-3 py-2 rounded-md ${
                isActive(apiName!) ? "bg-gray-200" : "hover:bg-gray-100"
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
              onClick={() => navigate(`/apis/${apiName}/settings`)}
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
              onClick={() => navigate(`/apis/${apiName}/newversion`)}
              className={`w-full text-left px-3 py-2 rounded-md ${
                isActive("newversion") ? "bg-gray-200" : "hover:bg-gray-100"
              }`}
            >
              <div className="flex items-center">
                <CircleFadingPlusIcon className="h-4 w-4 mr-2" />
                <span>New version</span>
              </div>
            </button>
          </li>
        </ul>
      </div>

      <div>
        <h2 className="text-sm font-medium text-gray-500 mb-4">Routes</h2>
        <button
          onClick={() => navigate(`/apis/${apiName}/routes/new`)}
          className="flex items-center space-x-2 text-sm text-gray-600 px-3 py-2 flex items-center justify-center w-full border border-gray-300 rounded-lg hover:text-gray-900 hover:border-gray-400"
        >
          <Plus className="h-4 w-4" />
          <span>Add</span>
        </button>
      </div>
    </nav>
  );
};

export default APILeftNav;
