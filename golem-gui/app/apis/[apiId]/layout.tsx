"use client"

import Sidebar from "@/components/ui/Sidebar";
import { useParams } from "next/navigation";

export default function APISLayout({
  children,
}: {
  children: React.ReactNode;
}) {

  const { id, apiId } = useParams();

  return (
    
    <div style={{ display: "flex", height: "100vh", overflow: "hidden" }}>
      <Sidebar apiId={Number(apiId)} />
      <div
        className="flex-1 p-[20px]"
        style={{
          overflowY: "auto",
          height: "100vh",
        }}
      >
        {children}
      </div>
    </div>
  );
}
