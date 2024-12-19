"use client"

import { AppBar, Toolbar } from "@mui/material";
import { ModeToggle } from "../toggle-button";
import Logo from "../../assets/golem-logo";
import { Box, List, ListItem, ListItemText } from "@mui/material";
import Link from "next/link";
import { usePathname } from 'next/navigation';

type NAV_LINK = {
  name: string;
  to: string;
};

const links = [
  { name: "Home", to: "/" },
  { name: "Overview", to: "/overview" },
  { name: "Components", to: "/components" },
  { name: "Workers", to: "/workers" },
  { name: "APIs", to: "/apis" },
  { name: "Plugins", to: "/plugins" },
] as NAV_LINK[];


export default function Navbar() {
  const pathname = usePathname();
  
  return (
    <AppBar
      position="static"
      color="transparent"
      className="dark:bg-[#0a0a0a] bg-white border-b border-gray-300 dark:border-[#3f3f3f]"
      sx={{ boxShadow: "0px 0px" }}
    >
      <Toolbar className="flex justify-between">
        <Logo />
        <List className="flex gap-4">
          {links.map((link) => (
            <Link key={link.name
            } href={link.to} style={{ textDecoration: "none", color: "inherit" }}>
              <ListItem
                sx={{
                  padding: "0.3rem 0.8rem", // Reduced padding for smaller background
                  marginBottom: "0.5rem", 
                  cursor: "pointer",
                  borderRadius: "3px",
                  borderBottom: pathname === link.to ? "1px solid #373737" : "transparent",
                  "&:hover": {
                    backgroundColor: "#373737",
                  },
                }}
                className={`dark:hover:bg-[#373737] hover:bg-[#C0C0C0]`}
              >
                <ListItemText primary={link.name} />
              </ListItem>
            </Link>
          ))}
        </List>
        <ModeToggle />
      </Toolbar>
    </AppBar>
  );
}
