import { Outlet } from "react-router-dom";
export default function PlaygroundLayout() {
  return (
    <div
      style={{
        overflow: "hidden",
        height: "100vh",
        margin: "auto",
      }}
    >
      {<Outlet />}
    </div>
  );
}
