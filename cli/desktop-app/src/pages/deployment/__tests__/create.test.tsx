import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import CreateDeployment from "../create";

// Mock React Router
vi.mock("react-router-dom", async () => {
  const actual = await vi.importActual("react-router-dom");
  return {
    ...actual,
    useNavigate: () => vi.fn(),
    useParams: () => ({ appId: "test-app-id" }),
  };
});

// Mock hooks
vi.mock("@/hooks/use-toast", () => ({
  useToast: () => ({ toast: vi.fn() }),
}));

// Mock service
vi.mock("@/service", () => ({
  API: {
    apiService: {
      getApiList: vi.fn().mockResolvedValue([]),
    },
    deploymentService: {
      createDeployment: vi.fn(),
    },
  },
}));

// Mock error boundary
vi.mock("@/components/errorBoundary", () => ({
  default: ({ children }: { children: React.ReactNode }) => (
    <div data-testid="error-boundary">{children}</div>
  ),
}));

// Mock UI components
vi.mock("@/components/ui/button", () => ({
  Button: ({ children }: { children: React.ReactNode }) => (
    <button>{children}</button>
  ),
}));

vi.mock("@/components/ui/input", () => ({
  Input: (props: React.InputHTMLAttributes<HTMLInputElement>) => (
    <input {...props} />
  ),
}));

vi.mock("@/components/ui/select", () => ({
  Select: ({ children }: { children: React.ReactNode }) => (
    <div>{children}</div>
  ),
  SelectContent: ({ children }: { children: React.ReactNode }) => (
    <div>{children}</div>
  ),
  SelectItem: ({ children }: { children: React.ReactNode }) => (
    <div>{children}</div>
  ),
  SelectTrigger: ({ children }: { children: React.ReactNode }) => (
    <div>{children}</div>
  ),
  SelectValue: ({ placeholder }: { placeholder?: string }) => (
    <span>{placeholder}</span>
  ),
}));

vi.mock("@/components/ui/card", () => ({
  Card: ({ children }: { children: React.ReactNode }) => <div>{children}</div>,
  CardContent: ({ children }: { children: React.ReactNode }) => (
    <div>{children}</div>
  ),
}));

vi.mock("@/components/ui/form", () => ({
  Form: ({ children }: { children: React.ReactNode }) => (
    <form>{children}</form>
  ),
  FormControl: ({ children }: { children: React.ReactNode }) => (
    <div>{children}</div>
  ),
  FormField: ({ render }: { render: Function }) => {
    const field = { name: "test", value: "", onChange: vi.fn() };
    return <div>{render({ field, fieldState: { error: null } })}</div>;
  },
  FormItem: ({ children }: { children: React.ReactNode }) => (
    <div>{children}</div>
  ),
  FormLabel: ({ children }: { children: React.ReactNode }) => (
    <label>{children}</label>
  ),
  FormMessage: ({ children }: { children: React.ReactNode }) => (
    <span>{children}</span>
  ),
  FormDescription: ({ children }: { children: React.ReactNode }) => (
    <p>{children}</p>
  ),
}));

vi.mock("react-hook-form", () => ({
  useForm: () => ({
    register: vi.fn(),
    handleSubmit: vi.fn(fn => (e: React.FormEvent) => {
      e?.preventDefault?.();
      return fn({ domain: "localhost:9006", definitions: [] });
    }),
    formState: { errors: {} },
    control: {},
    setError: vi.fn(),
    setValue: vi.fn(),
    getValues: vi.fn((field?: string) => {
      if (field === "definitions") return [];
      return { domain: "localhost:9006", definitions: [] };
    }),
    watch: vi.fn(() => []),
  }),
  useFieldArray: () => ({
    fields: [],
    append: vi.fn(),
    remove: vi.fn(),
  }),
}));

vi.mock("@hookform/resolvers/zod", () => ({
  zodResolver: vi.fn(),
}));

// Mock zod
vi.mock("zod", () => {
  const mockSchema = {
    min: vi.fn().mockReturnThis(),
    regex: vi.fn().mockReturnThis(),
    refine: vi.fn().mockReturnThis(),
    transform: vi.fn().mockReturnThis(),
  };

  return {
    z: {
      object: vi.fn(() => mockSchema),
      string: vi.fn(() => mockSchema),
      array: vi.fn(() => mockSchema),
    },
    object: vi.fn(() => mockSchema),
    string: vi.fn(() => mockSchema),
    array: vi.fn(() => mockSchema),
  };
});

vi.mock("lucide-react", () => ({
  Loader2: () => <span>Loading</span>,
  Plus: () => <span>+</span>,
  X: () => <span>×</span>,
}));

describe("CreateDeployment", () => {
  const renderCreateDeployment = () => {
    return render(
      <MemoryRouter>
        <CreateDeployment />
      </MemoryRouter>,
    );
  };

  it("should render without crashing", () => {
    expect(() => {
      renderCreateDeployment();
    }).not.toThrow();
  });

  it("should render within error boundary", () => {
    renderCreateDeployment();
    expect(screen.getByTestId("error-boundary")).toBeInTheDocument();
  });

  it("should render the page title", () => {
    renderCreateDeployment();
    expect(screen.getByText("Deploy API")).toBeInTheDocument();
  });

  it("should render the page description", () => {
    renderCreateDeployment();
    expect(
      screen.getByText(
        "Create a new deployment with one or more API definitions",
      ),
    ).toBeInTheDocument();
  });

  it("should render action buttons", () => {
    renderCreateDeployment();
    expect(screen.getByText("Deploy")).toBeInTheDocument();
  });
});
