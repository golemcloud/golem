import React from "react";
import { Link, useLocation } from "react-router-dom";

interface NavLinkProps {
  to: string;
  children: React.ReactNode;
}

const NavLink = ({ to, children }: NavLinkProps) => {
  const location = useLocation();
  const isActive =
    location.pathname === to || location.pathname.startsWith(to + "/");

  return (
    <Link
      to={to}
      className={`${
        isActive
          ? "bg-primary-background border-b-2 border-primary-soft text-primary-soft"
          : "text-gray-500 hover:text-gray-700"
      } py-2`}
    >
      {children}
    </Link>
  );
};

export default NavLink;
