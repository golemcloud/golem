"use client";
export default function APISLayout({
  children,
}: {
  children: React.ReactNode;
}) {
  return (
    <div
      style={{
        overflow: "hidden",
        height: "100vh",
        margin: "auto",
      }}
    >
      {children}
    </div>
  );
}
