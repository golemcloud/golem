import {
  SidebarGroup,
  SidebarGroupLabel,
  SidebarMenu,
  SidebarMenuButton,
  SidebarMenuItem,
} from "@/components/ui/sidebar";

import { Badge } from "@/components/ui/badge.tsx";
import { useNavigate } from "react-router-dom";

export const HTTP_METHOD_COLOR = {
  Get: "bg-emerald-700 text-emerald-100 hover:bg-emerald-800", // Softer Emerald
  Post: "bg-gray-800 text-gray-100 hover:bg-gray-900", // Dark Gray
  Put: "bg-amber-700 text-amber-100 hover:bg-amber-800", // Warmer Yellow
  Patch: "bg-blue-700 text-blue-100 hover:bg-blue-800", // Softer Blue
  Delete: "bg-red-700 text-red-100 hover:bg-red-800", // Softer Red
  Head: "bg-purple-700 text-purple-100 hover:bg-purple-800", // Softer Purple
  Options: "bg-indigo-700 text-indigo-100 hover:bg-indigo-800", // Softer Indigo
  Trace: "bg-pink-700 text-pink-100 hover:bg-pink-800", // Softer Pink
  Connect: "bg-sky-700 text-sky-100 hover:bg-sky-800", // Softer Sky Blue
};

export function NavRoutes({
  routes,
  setActiveItem,
  activeItem,
}: {
  routes: {
    method: string;
    name: string;
    url: string;
  }[];
  setActiveItem: (item: string) => void;
  activeItem: string;
}) {
  const navigate = useNavigate();

  return (
    <SidebarGroup className="group-data-[collapsible=icon]:hidden">
      <SidebarGroupLabel>Routes</SidebarGroupLabel>
      <SidebarMenu>
        {routes.map(item => (
          <SidebarMenuItem key={`${item.name}-${item.method}`}>
            <SidebarMenuButton
              onClick={() => {
                setActiveItem(item.name);
              }}
              isActive={item.name === activeItem}
              asChild
            >
              <div
                className="cursor-pointer"
                onClick={() => navigate(item.url)}
              >
                <Badge
                  variant="secondary"
                  className={
                    HTTP_METHOD_COLOR[
                      item.method as keyof typeof HTTP_METHOD_COLOR
                    ]
                  }
                >
                  {item.method}
                </Badge>
                <span>{item.name}</span>
              </div>
            </SidebarMenuButton>
          </SidebarMenuItem>
        ))}
      </SidebarMenu>
    </SidebarGroup>
  );
}
