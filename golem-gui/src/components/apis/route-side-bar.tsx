import useApiDefinitions from "@lib/hooks/use-api-definitons";
import { useCustomParam } from "@lib/hooks/use-custom-param";
import { ApiRoute } from "@lib/types/api";
import { ListItem, ListItemText, ListItemIcon, List } from "@mui/material";
import { Loader } from "lucide-react";
import { useLocation, useSearchParams } from "react-router-dom";
import { Button2 } from "../ui/button";
import {Link} from "react-router-dom";

export default function RouteSideBar() {
  const {pathname} = useLocation();
  const [params] = useSearchParams();
  const { apiId } = useCustomParam();
  const version = params.get("version");
  const { isLoading, getApiDefintion } = useApiDefinitions(apiId, version);
  const { data: apiDefintion } = (!isLoading && getApiDefintion()) || {};

  if (isLoading) {
    return <Loader />;
  }

  return (
    <div>
      <List>
        {apiDefintion?.routes.map((route: ApiRoute, index: number) => {
          const routeId = encodeURIComponent(`${route.path}|${route.method}`);
          return (
            <Link
              key={index}
              to={`/apis/${apiId}/${routeId}${
                version ? `?version=${version}` : ""
              }`}
              style={{ textDecoration: "none", color: "inherit" }}
            >
              <ListItem
                sx={{
                  marginBottom: "0.8rem",
                  cursor: "pointer",
                  backgroundColor:
                    pathname === `/apis/${apiId}/${routeId}`
                      ? "#373737"
                      : "transparent",
                  "&:hover": {
                    backgroundColor: "#373737",
                  },
                }}
                className={`dark:hover:bg-[#373737] hover:bg-[#C0C0C0] ${
                  pathname === `/apis/${apiId}/${routeId}`
                    ? "dark:bg-[#373737] bg-[#C0C0C0]"
                    : "transparent"
                }`}
              >
                <ListItemText className="break-all" primary={route.path} />
                <ListItemIcon sx={{ minWidth: 32, color: "inherit" }}>
                  <Button2 variant="success" size="xs">
                    {route.method}
                  </Button2>
                </ListItemIcon>
              </ListItem>
            </Link>
          );
        })}
      </List>
    </div>
  );
}
