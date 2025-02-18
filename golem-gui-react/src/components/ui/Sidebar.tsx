import React from "react";
import {
  Box,
  Button,
  List,
  ListItem,
  ListItemIcon,
  ListItemText,
  Typography,
} from "@mui/material";
import { Add } from "@mui/icons-material";
import {Link} from "react-router-dom";
import { useLocation, useNavigate, useSearchParams } from "react-router-dom";
import RouteSideBar from "@components/apis/route-side-bar";

type SidebarProps = {
  id: string;
  navigationLinks: NavigationLinks[];
  variant: string;
  version?: string;
  apiTab?: string;
  type?: string;
};

type NavigationLinks = {
  name: string;
  href: string;
  icon: React.ReactNode;
};

const Sidebar = ({ id, navigationLinks, variant, apiTab,type }: SidebarProps) => {
  const {pathname} = useLocation();
  const navigate = useNavigate();
  const [params] = useSearchParams();
  const version = params.get("version");

  if(type === "Ephemeral"){
    navigationLinks = navigationLinks.filter((link) => link.name !== "Workers");
  }
  

  return (
    <Box
      sx={{
        width: 250,
        flexDirection: "column",
        padding: 2,
        minHeight: "100vh",
        display: apiTab === "playground" ? "none" : { xs: "none", md: "flex" },
      }}
      className=" bg-primary border-r "
    >
      {variant == "apis" && (
        <Typography
          variant="subtitle2"
          sx={{
            fontWeight: "bold",
            color: "#AAAAAA",
            fontSize: "14px",
          }}
        >
          API
        </Typography>
      )}
      <List>
        {navigationLinks.map((link) => {
          const isActive =
            pathname === link.href ||
            (link.href !== "/" && pathname.startsWith(link.href));
          return (
            <Link
              key={link.name}
              to={link.href}
              style={{ textDecoration: "none", color: "inherit" }}
            >
              <ListItem
                sx={{
                  marginBottom: "0.8rem",
                  cursor: "pointer",
                  borderRadius: "10px",
                }}
                className={`hover:bg-silver
                ${isActive ? "bg-silver" : "transparent"}
                `}
              >
                <ListItemIcon sx={{ minWidth: 32, color: "inherit" }}>
                  {link.icon}
                </ListItemIcon>
                <ListItemText primary={link.name} />
              </ListItem>
            </Link>
          );
        })}
      </List>
      {variant === "apis" && (
        <>
          <Typography
            variant="subtitle2"
            sx={{
              fontWeight: "bold",
              color: "#AAAAAA",
              marginBottom: 1,
              fontSize: "14px",
            }}
          >
            Routes
          </Typography>
          <RouteSideBar/>
        </>
      )}

      {variant === "apis" && (
        //TODO:for now handling for button. but needs to Link.(don't want to break the ui)
        <Button
          onClick={(e) => {
            e.preventDefault();
            navigate(
              `/apis/${id}/new-route${version ? `?version=${version}` : ""}`
            );
          }}
          variant="outlined"
          sx={{
            textTransform: "none",
            padding: "6px 12px",
            fontSize: "16px",
            fontWeight: "500",
          }}
          fullWidth
          className="border border-border dark:hover:bg-[#333] hover:bg-[#c0c0c0] text-foreground"
        >
          Add
          <Add className="ml-2" />
        </Button>
      )}
    </Box>
  );
};

export default Sidebar;
