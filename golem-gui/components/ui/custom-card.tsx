import { Box, Card, CardContent, Typography, Chip } from "@mui/material";

interface CustomCardProps {
    title: string;
    timestamp: string;
    tags: string[];
    version: string;
  }
const CustomCard = ({ title, timestamp, tags, version }:CustomCardProps) => {
  return (
    <Card
      sx={{
        width: 300,
        backgroundColor: "#1c1c1c",
        color: "#ffffff",
        borderRadius: 2,
        padding: 2,
        position: "relative",
      }}
    >
      <CardContent>
        <Typography variant="h6" component="div" gutterBottom>
          {title}
        </Typography>
        <Typography
          variant="body2"
          sx={{ color: "gray", marginBottom: 2 }}
        >
          {timestamp}
        </Typography>
        <Box sx={{ display: "flex", gap: 1, flexWrap: "wrap" }}>
          {tags.map((tag, index) => (
            <Chip
              key={index}
              label={tag}
              sx={{
                backgroundColor: "#333",
                color: "#fff",
              }}
            />
          ))}
        </Box>
      </CardContent>
      <Box
        sx={{
          position: "absolute",
          top: 8,
          right: 8,
          backgroundColor: "#333",
          color: "#fff",
          borderRadius: 1,
          padding: "2px 6px",
          fontSize: "0.8rem",
        }}
      >
        {version}
      </Box>
    </Card>
  );
};

export default CustomCard;
