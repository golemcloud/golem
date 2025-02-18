import { Box, Container, Typography, Link, Stack, IconButton } from "@mui/material";
import GitHubIcon from "@mui/icons-material/GitHub";
import TwitterIcon from "@mui/icons-material/Twitter";
import MailOutlineIcon from "@mui/icons-material/MailOutline";
import SportsEsportsIcon from "@mui/icons-material/SportsEsports";
import Logo from '../assets/GolemLogo';

const Footer = () => {
  return (
    <Box
      component="footer"
      className="bg-primary border-t border-border py-20"
    >
      <Container
        maxWidth="lg"
        sx={{
          display: "flex",
          justifyContent: "space-between",
          alignItems: "flex-start",
          flexWrap: "wrap",
          rowGap: 4,
        }}
      >
        <Box
          sx={{
            flexBasis: { xs: "100%", sm: "45%", md: "30%" },
          }}
        >
          <Typography
            variant="h6"
            sx={{
              fontWeight: "bold",
              marginBottom: 1,
            }}
          >
            <Logo />
          </Typography>
          <Stack direction="row" spacing={1} marginBottom={1}>
            <IconButton
             color="inherit"
              sx={{  border: "1px solid #333", borderRadius: "8px" }}
            >
              <GitHubIcon />
            </IconButton>
            <IconButton
              color="inherit"
              sx={{  border: "1px solid #333", borderRadius: "8px" }}
            >
              <TwitterIcon />
            </IconButton>
            <IconButton
              color="inherit"
              sx={{  border: "1px solid #333", borderRadius: "8px" }}
            >
              <MailOutlineIcon />
            </IconButton>
            <IconButton
              color="inherit"
              sx={{ border: "1px solid #333", borderRadius: "8px" }}
            >
              <SportsEsportsIcon />
            </IconButton>
          </Stack>
          <Typography variant="caption" color="inherit">
            © 2024 Zverge Inc.
          </Typography>
        </Box>

        {/* Middle Section */}
        <Box
          sx={{
            flexBasis: { xs: "100%", sm: "25%", md: "20%" }, 
          }}
        >
          <Typography
            variant="subtitle2"
            sx={{ fontWeight: "bold", marginBottom: 1 }}
          >
            Golem
          </Typography>
          <Stack spacing={0.5}>
            <Link href="https://www.golem.cloud/" color="inherit" underline="hover"
             target="_blank" 
             rel="noopener noreferrer"
            >
              About
            </Link>
            <Link href="https://learn.golem.cloud/" color="inherit" underline="hover"
             target="_blank" 
             rel="noopener noreferrer"
            >
              Docs
            </Link>
          </Stack>
        </Box>

        <Box
          sx={{
            flexBasis: { xs: "100%", sm: "25%", md: "20%" },
          }}
        >
          <Typography
            variant="subtitle2"
            sx={{ fontWeight: "bold", marginBottom: 1 }}
          >
            Support
          </Typography>
          <Stack spacing={0.5}>
            <Link href="https://www.golem.cloud/blog" color="inherit" underline="hover"
             target="_blank"
             rel="noopener noreferrer"
            >
              Blog
            </Link>
            <Link href="https://support.golem.cloud/" color="inherit" underline="hover"
             target="_blank"
             rel="noopener noreferrer"
            >
              Help Center
            </Link>
          </Stack>
        </Box>
      </Container>
    </Box>
  );
};

export default Footer;
