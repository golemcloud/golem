"use client"; 

import React from "react";
import { Box, Button, List, ListItem, ListItemIcon, ListItemText, Typography } from "@mui/material";
import { Home, Settings, RocketLaunch, Add } from "@mui/icons-material";
import Link from "next/link";
import { usePathname, useSearchParams } from "next/navigation";
import PlayForWorkIcon from '@mui/icons-material/PlayForWork';

type SidebarProps = {
  id: string;
  navigationLinks:NavigationLinks[],
  variant:string;
  version?: string;     
};

type NavigationLinks = {
  name: string;
  href: string;
  icon: React.ReactNode;
};

const Sidebar = ({ id, navigationLinks, variant, version }: SidebarProps) => {

  const pathname = usePathname();
  const searchParams = useSearchParams();
  version = searchParams?.get("version") || version       

  return (
    <Box
      sx={{
        width: 250,
        height: "100vh",
        display: "flex",
        flexDirection: "column",
        padding: 2,
      }}
      className="dark:bg-[#0a0a0a] bg-white border-r border-gray-300 dark:border-[#3f3f3f]"
    >
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

      <List>
        {navigationLinks.map((link) => (
          <Link key={link.name} href={link.href} style={{ textDecoration: "none", color: "inherit" }}>
            <ListItem
              sx={{
                 marginBottom: "0.8rem",
                cursor: "pointer",
                borderRadius: "10px",
                backgroundColor: pathname === link.href ? "#373737" : "transparent",
                "&:hover": {
                  backgroundColor: "#373737",
                },
              }}
              className={`dark:hover:bg-[#373737] hover:bg-[#C0C0C0]
                ${
                  pathname === link.href ? "dark:bg-[#373737] bg-[#C0C0C0]" : "transparent"
                }
                `}
            >
              <ListItemIcon sx={{ minWidth: 32, color: "inherit" }}>{link.icon}</ListItemIcon>
              <ListItemText primary={link.name} />
            </ListItem>
          </Link>
        ))}
      </List>

      <Typography
        variant="subtitle2"
        sx={{
          fontWeight: "bold",
          color: "#AAAAAA",
          marginTop: 3,
          marginBottom: 1,
          fontSize: "14px",
        }}
      >
        Routes
      </Typography>
      {variant==="apis" && <Link href={`/apis/${id}/new-route?version=${version}`}>
        <Button
        variant="outlined"
        sx={{
            textTransform: "none",
            padding: "6px 12px",
            fontSize: "16px",
            fontWeight: "500",
        }}
        fullWidth
        className="border  border-black dark:border-white text-black dark:text-white dark:hover:bg-[#373737] hover:bg-[#C0C0C0]"
        >
        Add
        < Add className="ml-2" />
        </Button>
      </Link>}
    </Box>
  );
};

export default Sidebar;