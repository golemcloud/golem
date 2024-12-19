"use client"; // For Next.js app directory

import React from "react";
import { Box, Container, Typography, Link, Stack, IconButton } from "@mui/material";
import GitHubIcon from "@mui/icons-material/GitHub";
import TwitterIcon from "@mui/icons-material/Twitter";
import MailOutlineIcon from "@mui/icons-material/MailOutline";
import SportsEsportsIcon from "@mui/icons-material/SportsEsports";
import Logo from '../../assets/golem-logo';


const Footer = () => {
  return (
    <Box
      component="footer"
      className="dark:bg-[#0a0a0a] bg-white border-t border-gray-300 dark:border-[#3f3f3f] py-20"
    >
      <Container
        maxWidth="lg"
        sx={{
          display: "flex",
          justifyContent: "space-between",
          alignItems: "flex-start",
          flexWrap: "wrap",
        }}
      >
        {/* Left Section */}
        <Box>
          <Typography
            variant="h6"
            sx={{
              fontWeight: "bold",
              marginBottom: 1,
            }}
          >
            <Logo/>
          </Typography>
          <Stack direction="row" spacing={1} marginBottom={1}>
            <IconButton
              sx={{ color: "#FFFFFF", border: "1px solid #333", borderRadius: "8px" }}
            >
              <GitHubIcon />
            </IconButton>
            <IconButton
              sx={{ color: "#FFFFFF", border: "1px solid #333", borderRadius: "8px" }}
            >
              <TwitterIcon />
            </IconButton>
            <IconButton
              sx={{ color: "#FFFFFF", border: "1px solid #333", borderRadius: "8px" }}
            >
              <MailOutlineIcon />
            </IconButton>
            <IconButton
              sx={{ color: "#FFFFFF", border: "1px solid #333", borderRadius: "8px" }}
            >
              <SportsEsportsIcon />
            </IconButton>
          </Stack>
          <Typography variant="caption" sx={{ color: "#AAAAAA" }}>
            © 2024 Zverge Inc.
          </Typography>
        </Box>

        {/* Middle Section */}
        <Box>
          <Typography
            variant="subtitle2"
            sx={{ fontWeight: "bold", marginBottom: 1 }}
          >
            Golem
          </Typography>
          <Stack spacing={0.5}>
            <Link href="#" color="inherit" underline="hover">
              About
            </Link>
            <Link href="#" color="inherit" underline="hover">
              Docs
            </Link>
          </Stack>
        </Box>

        {/* Right Section */}
        <Box>
          <Typography
            variant="subtitle2"
            sx={{ fontWeight: "bold", marginBottom: 1 }}
          >
            Support
          </Typography>
          <Stack spacing={0.5}>
            <Link href="#" color="inherit" underline="hover">
              Blog
            </Link>
            <Link href="#" color="inherit" underline="hover">
              Help Center
            </Link>
          </Stack>
        </Box>
      </Container>
    </Box>
  );
};

export default Footer;
