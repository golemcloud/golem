"use client";

import React from "react";
import { Box, Typography, Card, Grid2 as MuiGrid } from "@mui/material";
import Link from "next/link";

interface Resource {
  label: string;
  icon: JSX.Element;
  description: string;
  link?: string;
}
interface ResourcesProps {
  resources: Resource[];
  variant: "main" | "others";
}

export default function FooterLinks({ resources, variant }: ResourcesProps) {
  return (
    <Box>
      {variant === "main" && (
        <Typography variant="h5" sx={{ mb: 3, fontWeight: 600 }}>
          Resources
        </Typography>
      )}
      <MuiGrid container spacing={1}>
        {resources.map((resource: Resource) => (
          <MuiGrid size={{ xs: 12, sm: 6, md: 6, lg: 3 }} key={resource.label}>
            <Link
              href={resource.link || "#"}
              style={{ textDecoration: "none", color: "inherit" }}
              target="_blank" // Opens the link in a new tab
              rel="noopener noreferrer" // Improves security by preventing access to the window.opener object
            >
              <Card
                sx={{
                  display: "flex",
                  flexDirection: "column",
                  justifyContent: "space-between",
                  padding: "3rem",
                  width: "100%",
                  height: "200px",
                  borderRadius: "5px",
                  transition: "transform 0.3s ease",
                  "&:hover": {
                    transform: "translateY(-5px)",
                  },
                }}
                className="border"
              >
                <Box
                  sx={{
                    display: "flex",
                    alignItems: "center",
                    justifyContent: "space-between",
                  }}
                >
                  <Typography variant="h6" sx={{ fontWeight: 600 }}>
                    {resource.label}
                  </Typography>
                  <Typography sx={{ fontSize: "2.5rem" }}>
                    {resource.icon}
                  </Typography>
                </Box>
                <Typography variant="body2" className="text-muted-foreground">
                  {resource.description}
                </Typography>
              </Card>
            </Link>
          </MuiGrid>
        ))}
      </MuiGrid>
    </Box>
  );
}
