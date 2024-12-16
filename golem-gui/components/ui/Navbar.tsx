import { AppBar, Toolbar } from "@mui/material";
import { ModeToggle } from "../toggle-button";
import Logo from "../../assets/golem-logo";
import { Box, List, ListItem, ListItemText } from "@mui/material";
import Link from "next/link";

type NAV_LINK = {
  name: string;
  to: string;
};

const links = [
  { name: "Overview", to: "/overview" },
  { name: "Components", to: "/components" },
  { name: "Workers", to: "/workers" },
  { name: "APIs", to: "/projects" },
  { name: "Plugins", to: "/plugins" },
] as NAV_LINK[];

export default function Navbar() {
  return (
    <AppBar
      position="static"
      color="transparent"
      className="dark:bg-[#0a0a0a] bg-white border-b border-gray-300 dark:border-[#3f3f3f]"
      sx={{ boxShadow: "0px 0px" }}
    >
      <Toolbar className="flex justify-between">
        <Logo />
        <Box>
          <List className="flex">
            {links.map((link) => {
              return (
                <ListItem key={link.name}>
                  <Link href={link.to}>
                    <ListItemText primary={link.name} />
                  </Link>
                </ListItem>
              );
            })}
          </List>
        </Box>
        <ModeToggle />
      </Toolbar>
    </AppBar>
  );
}
