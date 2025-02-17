import { useState } from "react";
import {
  Toolbar,
  IconButton,
  Drawer,
  List,
  ListItemText,
  ListItem,
  Box,
} from "@mui/material";
import { Menu as MenuIcon } from "@mui/icons-material";
import { ModeToggle } from "./ui/toggle-button";
import Logo from "../assets/GolemLogo";
import { Link, useLocation } from "react-router-dom";

type NAV_LINK = {
  name: string;
  to: string;
  comingSoon?: boolean;
};

const links = [
  { name: "Home", to: "/" },
  { name: "Overview", to: "/overview" },
  { name: "Components", to: "/components" },
  { name: "APIs", to: "/apis" },
  { name: "Plugins", to: "/plugins", comingSoon: false },
] as NAV_LINK[];

export default function Navbar() {
  const location = useLocation();
  const [drawerOpen, setDrawerOpen] = useState(false);

  const toggleDrawer = (open: boolean) => () => {
    setDrawerOpen(open);
  };

  return (
    <Box
      position="static"
      className="bg-primary-background border-b border-border"
    >
      <Toolbar className="flex justify-between items-center">
        <Logo />
        <Box sx={{ display: { xs: "none", md: "flex" }, gap: 4 }}>
          {links.map((link) => {
            const isActive =
              location.pathname === link?.to ||
              (link.to !== "/" && location.pathname.startsWith(link.to));
            return (
              <Link
                key={link.name}
                to={link.comingSoon ? "#" : link.to}
                style={{ textDecoration: "none", color: "inherit" }}
              >
                <ListItem
                  sx={{
                    padding: "0.3rem 0.8rem",
                    marginBottom: "0.5rem",
                    cursor: "pointer",
                  }}
                  className={`hover:bg-silver rounded ${isActive ? "border-b-2 border-silver" : ""}`}
                >
                  <ListItemText
                    primary={`${link.name}${link.comingSoon ? "" : ""}`}
                  />
                </ListItem>
              </Link>
            );
          })}
        </Box>
        <Box sx={{ display: "flex", alignItems: "center", gap: 2 }}>
          <IconButton
            edge="start"
            color="inherit"
            aria-label="menu"
            sx={{ display: { xs: "block", md: "none" } }}
            onClick={toggleDrawer(true)}
          >
            <MenuIcon />
          </IconButton>
          <ModeToggle />
        </Box>
      </Toolbar>
      <Drawer
        anchor="right"
        open={drawerOpen}
        onClose={toggleDrawer(false)}
        PaperProps={{
          className:
            "dark:bg-[#0a0a0a] bg-white p-4 border-r border-gray-300 dark:border-[#3f3f3f]",
          sx: {
            width: 250,
          },
        }}
      >
        <List>
          {links.map((link) => {
            const isActive =
              location.pathname === link?.to ||
              (link.to !== "/" && location.pathname.startsWith(link.to));
            return (
              <Link
                key={link.name}
                to={link.comingSoon ? "#" : link.to}
                style={{ textDecoration: "none", color: "inherit" }}
              >
                <ListItem
                  sx={{
                    padding: "0.8rem 1.2rem",
                    borderBottom: isActive
                      ? "1px solid #373737"
                      : "transparent",
                  }}
                  className={`dark:hover:bg-[#373737] hover:bg-[#C0C0C0]`}
                  onClick={toggleDrawer(false)}
                >
                  <ListItemText
                    primary={`${link.name}${
                      link.comingSoon ? " (Coming Soon)" : ""
                    }`}
                  />
                </ListItem>
              </Link>
            );
          })}
        </List>
      </Drawer>
    </Box>
  );
}