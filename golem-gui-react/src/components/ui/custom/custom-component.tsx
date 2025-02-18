import React from "react";
import { ReactNode, ReactElement } from 'react';


interface Props {
  children: React.ReactNode;
  className?: string;
  sx?: React.CSSProperties;
}

interface TypographyProps {
  children: React.ReactNode;
  variant?: "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "body" | "caption" | "subtitle" | "subtitle2";
  className?: string;
  sx?: React.CSSProperties;
}


interface GridProps {
  children: ReactNode;
  container?: boolean;
  spacing?: number;
  className?: string;
  sx?: React.CSSProperties;
  size?: {
    xs?: number;
    sm?: number;
    md?: number;
    lg?: number;
    xl?: number;
  };
}

interface BoxProps {
  children: React.ReactNode;
  className?: string;
  sx?: React.CSSProperties;
  [key: string]: any;
}

interface PaperProps {
  children: React.ReactNode;
  className?: string;
  sx?: React.CSSProperties;
  elevation?: number;
}

export const Paper: React.FC<PaperProps> = ({
  children,
  className,
  sx,
  elevation = 1,
}) => {
  const elevationClass = `shadow-${elevation}`;

  // @ts-expect-error - We are removing the margin, padding and other spacing props from the sx prop
  const { mb, py, px, ...restSx } = sx || {};

  const spacingClasses = `${mb ? `mb-${mb}` : ""} ${py ? `py-${py}` : ""} ${px ? `px-${px}` : ""}`;

  return (
    <div
      className={`dark:bg-[#222] text-foreground p-4 m-4 w-full ${elevationClass} ${spacingClasses} ${className}`}
      style={restSx}
    >
      {children}
    </div>
  );
};



export const Card: React.FC<Props> = ({
  children,
  className,
  sx,
}) => {
  return (
    <div
      className={`bg-white rounded-lg shadow-lg p-6 m-4 w-full max-w-xl ${className}`}
      style={sx}
    >
      {children}
    </div>
  );
};

export const Typography: React.FC<TypographyProps> = ({
  children,
  variant = "body",
  className,
  sx,
}) => {
  const Element =
    variant === "h1"
      ? "h1"
      : variant === "h2"
      ? "h2"
      : variant === "h3"
      ? "h3"
      : variant === "h4"
      ? "h4"
      : variant === "h5"
      ? "h5"
      : variant === "h6"
      ? "h6"
      : variant === "caption"
      ? "span"
      : variant === "subtitle"
      ? "p"
      : variant === "subtitle2"
      ? "p"
      : "p"; // Default to p if not matched

  return (
    <Element
      className={`${variant === "h1" ? "text-4xl" : ""}
        ${variant === "h2" ? "text-3xl" : ""} ${
        variant === "h3" ? "text-2xl" : ""
      } ${variant === "h4" ? "text-xl" : ""} ${
        variant === "h5" ? "text-lg" : ""
      } ${variant === "h6" ? "text-base" : ""} ${
        variant === "body" ? "text-base" : ""
      } ${variant === "caption" ? "text-sm" : ""} ${
        variant === "subtitle" ? "text-lg" : ""
      } ${variant === "subtitle2" ? "text-sm" : ""}
        ${className}`}
      style={sx}
    >
      {children}
    </Element>
  );
};


export const Grid: React.FC<GridProps> = ({
  children,
  container = false,
  spacing = 4,
  className,
  sx,
  size,
}) => {
  const gridContainerClass = container
    ? `grid grid-cols-1 gap-${spacing} ${className}`
    : `${className}`;

  const gridItemClass = size
    ? `col-span-${size.xs || 1} sm:col-span-${size.sm || 1} md:col-span-${size.md || 1} lg:col-span-${size.lg || 1} xl:col-span-${size.xl || 1}`
    : 'col-span-1';

  return (
    <div
      className={gridContainerClass}
      style={sx}
    >
      {React.Children.map(children, (child) => {
        // Ensure the child is a valid React element with props
        if (React.isValidElement(child)) {
          // Forcefully type the child as ReactElement with className
          const childWithClassName = child as ReactElement<{ className?: string }>;

          return React.cloneElement(childWithClassName, {
            className: `${childWithClassName.props.className} ${gridItemClass}`,
          });
        }
        return child;
      })}
    </div>
  );
};


export const Box: React.FC<BoxProps> = ({ children, className = '', sx, ...props }) => {
  return (
    <div className={className} style={sx} {...props}>
      {children}
    </div>
  );
};
