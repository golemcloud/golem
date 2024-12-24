"use client";

import { useState } from "react";
import {
  AppBar,
  Toolbar,
  IconButton,
  Drawer,
  List,
  ListItem,
  ListItemText,
  Box,
} from "@mui/material";
import { Menu as MenuIcon } from "@mui/icons-material";
import { ModeToggle } from "../toggle-button";
import Logo from "../../assets/golem-logo";
import Link from "next/link";
import { usePathname } from "next/navigation";

type NAV_LINK = {
  name: string;
  to: string;
  comingSoon?: boolean;
};

const links = [
  { name: "Home", to: "/" },
  { name: "Overview", to: "/overview" },
  { name: "Components", to: "/components" },
  { name: "Workers", to: "/workers" },
  { name: "APIs", to: "/apis" },
  { name: "Plugins", to: "/plugins", comingSoon: true },
] as NAV_LINK[];

export default function Navbar() {
  const pathname = usePathname();
  const [drawerOpen, setDrawerOpen] = useState(false);

  const toggleDrawer = (open: boolean) => () => {
    setDrawerOpen(open);
  };

  return (
    <AppBar
      position="static"
      color="transparent"
      className="dark:bg-[#0a0a0a] bg-white border-b border-gray-300 dark:border-[#3f3f3f]"
      sx={{ boxShadow: "0px 0px" }}
    >
      <Toolbar className="flex justify-between items-center">
        {/* Logo */}
        <Logo />

        {/* Desktop Navigation */}
        <Box sx={{ display: { xs: "none", md: "flex" }, gap: 4 }}>
          {links.map((link) => (
            <Link
              key={link.name}
              href={link.comingSoon ? "#" : link.to}
              style={{ textDecoration: "none", color: "inherit" }}
            >
              <ListItem
                sx={{
                  padding: "0.3rem 0.8rem",
                  marginBottom: "0.5rem",
                  cursor: "pointer",
                  borderRadius: "3px",
                  borderBottom:
                    pathname === link.to ? "1px solid #373737" : "transparent",
                  "&:hover": {
                    backgroundColor: "#373737",
                  },
                }}
                className={`dark:hover:bg-[#373737] hover:bg-[#C0C0C0]`}
              >
                <ListItemText
                  primary={`${link.name}${link.comingSoon ? "" : ""}`}
                />
              </ListItem>
            </Link>
          ))}
        </Box>

        {/* Mobile Menu and Dark Mode Toggle */}
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

      {/* Mobile Drawer */}
      <Drawer
        anchor="right"
        open={drawerOpen}
        onClose={toggleDrawer(false)}
        PaperProps={{
          sx: {
            width: 250,
            backgroundColor: "background.default",
          },
        }}
      >
        <List>
          {links.map((link) => (
            <Link
              key={link.name}
              href={link.comingSoon ? "#" : link.to}
              style={{ textDecoration: "none", color: "inherit" }}
            >
              <ListItem
                button
                sx={{
                  padding: "0.8rem 1.2rem",
                  borderBottom:
                    pathname === link.to ? "1px solid #373737" : "transparent",
                  "&:hover": {
                    backgroundColor: "#f0f0f0",
                  },
                }}
                onClick={toggleDrawer(false)}
              >
                <ListItemText
                  primary={`${link.name}${
                    link.comingSoon ? " (Coming Soon)" : ""
                  }`}
                />
              </ListItem>
            </Link>
          ))}
        </List>
      </Drawer>
    </AppBar>
  );
}
