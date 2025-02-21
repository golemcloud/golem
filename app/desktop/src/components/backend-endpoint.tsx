import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { API, updateService } from "@/service";
import { Settings } from "lucide-react";
import { useState } from "react";
import { z } from "zod";

// Helper function to validate IP address
const isValidIpAddress = (ip: string) => {
  const parts = ip.split(".");
  return (
    parts.length === 4 &&
    parts.every(part => {
      const num = Number.parseInt(part, 10);
      return num >= 0 && num <= 255 && part === num.toString();
    })
  );
};

// Helper function to validate domain
const isValidDomain = (domain: string) => {
  return /^[a-zA-Z0-9]+([-.][a-zA-Z0-9]+)*\.[a-zA-Z]{2,}$/.test(domain);
};

// Zod schema for endpoint validation
const endpointSchema = z.string().refine(
  value => {
    try {
      const url = new URL(value);
      if (url.protocol !== "http:" && url.protocol !== "https:") {
        return false;
      }
      if (url.hostname === "localhost") {
        return !!url.port; // Ensure port is specified for localhost
      }
      if (/^\d+(\.\d+){0,3}$/.test(url.hostname)) {
        return isValidIpAddress(url.hostname);
      }
      return isValidDomain(url.hostname);
    } catch {
      return false;
    }
  },
  {
    message: "Invalid endpoint format",
  },
);

export function BackendEndpointInput() {
  const [endpoint, setEndpoint] = useState(API.baseUrl);
  const [open, setOpen] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const handleSave = async () => {
    try {
      endpointSchema.parse(endpoint);
      await updateService(endpoint);
      window.location.reload();
      setOpen(false);
      setError(null);
    } catch (err) {
      if (err instanceof z.ZodError) {
        // Provide more specific error messages
        if (
          !endpoint.startsWith("http://") &&
          !endpoint.startsWith("https://")
        ) {
          setError("Endpoint must start with http:// or https://");
        } else {
          try {
            const url = new URL(endpoint);
            if (url.hostname === "localhost") {
              if (!url.port) {
                setError("Invalid localhost format. Use http://localhost:port");
              }
            } else if (/^\d+(\.\d+){0,3}$/.test(url.hostname)) {
              if (!isValidIpAddress(url.hostname)) {
                setError(
                  "Invalid IP address. Use format: xxx.xxx.xxx.xxx where each xxx is a number between 0 and 255",
                );
              }
            } else if (!isValidDomain(url.hostname)) {
              setError(
                "Invalid domain format. Use http://domain.com or http://subdomain.domain.com",
              );
            } else {
              setError(null);
            }
          } catch {
            setError("Invalid URL format");
          }
        }
      } else {
        setError("An unexpected error occurred");
      }
    }
  };

  return (
    <Dialog open={open} onOpenChange={setOpen}>
      <DialogTrigger asChild>
        <Button variant="outline" size="icon">
          <Settings className="h-4 w-4" />
        </Button>
      </DialogTrigger>
      <DialogContent className="sm:max-w-[425px]">
        <DialogHeader>
          <DialogTitle>Backend Endpoint</DialogTitle>
          <DialogDescription>
            Set the backend endpoint for your application.
          </DialogDescription>
        </DialogHeader>
        <div className="grid gap-4 py-4">
          <div className="grid grid-cols-4 items-center gap-4">
            <Label htmlFor="endpoint" className="text-right">
              Endpoint
            </Label>
            <Input
              id="endpoint"
              value={endpoint}
              onChange={e => setEndpoint(e.target.value)}
              className="col-span-3"
            />
          </div>
          {error && <p className="text-sm text-red-500">{error}</p>}
        </div>
        <DialogFooter>
          <Button type="submit" onClick={handleSave}>
            Save changes
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
