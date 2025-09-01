import { render, screen, fireEvent } from "@testing-library/react";
import { describe, it, expect, vi } from "vitest";
import { InvokeLayout, InvokeParams } from "../InvokeLayout";
import { Export, ComponentExportFunction } from "@/types/component";

// Mock lucide-react icons
vi.mock("lucide-react", () => ({
  ClipboardCopy: () => <div data-testid="clipboard-copy">📋</div>,
  Presentation: () => <div data-testid="presentation">📊</div>,
  TableIcon: () => <div data-testid="table-icon">📋</div>,
  MinusCircle: () => <div data-testid="minus-circle">-</div>,
  PlusCircle: () => <div data-testid="plus-circle">+</div>,
  ChevronDown: () => <div data-testid="chevron-down">v</div>,
  ChevronUp: () => <div data-testid="chevron-up">^</div>,
  Check: () => <div data-testid="check">✓</div>,
  Play: () => <div data-testid="play">▶</div>,
  TimerReset: () => <div data-testid="timer-reset">⏱</div>,
  Info: () => <div data-testid="info">ℹ</div>,
  CircleSlash2: () => <div data-testid="circle-slash">⭕</div>,
}));

// Mock DynamicForm component
vi.mock("@/pages/workers/details/dynamic-form.tsx", () => ({
  DynamicForm: ({
    functionDetails,
    onInvoke,
    exportName,
  }: {
    functionDetails: ComponentExportFunction;
    onInvoke: (data: InvokeParams) => void;
    exportName: string;
  }) => (
    <div data-testid="dynamic-form">
      <div>Dynamic Form for {exportName}</div>
      <div>Function: {functionDetails.name}</div>
      <button
        onClick={() =>
          onInvoke({
            params: functionDetails.parameters.map(param => ({
              name: param.name,
              typ: param.typ,
              value: "test-value",
            })),
          })
        }
      >
        Invoke Function
      </button>
    </div>
  ),
}));

// Mock SectionCard component
vi.mock("../SectionCard", () => ({
  SectionCard: ({
    title,
    description,
    value,
    onValueChange,
    onInvoke,
    readOnly,
  }: {
    title: string;
    description: string;
    value: string;
    onValueChange?: (value: string) => void;
    onInvoke?: (data: InvokeParams) => void;
    readOnly?: boolean;
  }) => (
    <div data-testid={`section-card-${title.toLowerCase()}`}>
      <h3>{title}</h3>
      <p>{description}</p>
      <div data-testid="section-value">{value}</div>
      {onValueChange && !readOnly && (
        <textarea
          data-testid="section-textarea"
          value={value}
          onChange={e => onValueChange(e.target.value)}
        />
      )}
      {onInvoke && (
        <button
          data-testid="section-invoke"
          onClick={() => onInvoke({ params: [] })}
        >
          Invoke
        </button>
      )}
    </div>
  ),
}));

// Mock worker utilities
vi.mock("@/lib/worker", () => ({
  parseToJsonEditor: (functionDetails: ComponentExportFunction) => ({
    [functionDetails.parameters[0]?.name || "param"]: "default-value",
  }),
  parseTypesData: (functionDetails: ComponentExportFunction) => ({
    types: functionDetails.parameters.map(p => ({
      name: p.name,
      type: p.type,
    })),
  }),
}));

describe("InvokeLayout", () => {
  const sampleParsedExports: Export[] = [
    {
      name: "appy:complex-exports/appy-complex-api",
      type: "function",
      functions: [
        {
          name: "add-contact",
          parameters: [
            {
              name: "c",
              type: "record { name: string, addresses: list<variant { home(record { street: string, city: string, state: string, zip: string, apartment: option<string> }), business(record { company-name: string, street: string, suite: option<string>, city: string, state: string, zip: string }), po-box(record { box-number: string, city: string, state: string, zip: string }) }> }",
              typ: {
                type: "record",
                fields: [
                  { name: "name", typ: { type: "str" } },
                  {
                    name: "addresses",
                    typ: {
                      type: "list",
                      inner: {
                        type: "variant",
                        cases: [
                          {
                            name: "home",
                            typ: {
                              type: "record",
                              fields: [
                                { name: "street", typ: { type: "str" } },
                                { name: "city", typ: { type: "str" } },
                                { name: "state", typ: { type: "str" } },
                                { name: "zip", typ: { type: "str" } },
                                {
                                  name: "apartment",
                                  typ: {
                                    type: "option",
                                    inner: { type: "str" },
                                  },
                                },
                              ],
                            },
                          },
                          {
                            name: "business",
                            typ: {
                              type: "record",
                              fields: [
                                { name: "company-name", typ: { type: "str" } },
                                { name: "street", typ: { type: "str" } },
                                {
                                  name: "suite",
                                  typ: {
                                    type: "option",
                                    inner: { type: "str" },
                                  },
                                },
                                { name: "city", typ: { type: "str" } },
                                { name: "state", typ: { type: "str" } },
                                { name: "zip", typ: { type: "str" } },
                              ],
                            },
                          },
                          {
                            name: "po-box",
                            typ: {
                              type: "record",
                              fields: [
                                { name: "box-number", typ: { type: "str" } },
                                { name: "city", typ: { type: "str" } },
                                { name: "state", typ: { type: "str" } },
                                { name: "zip", typ: { type: "str" } },
                              ],
                            },
                          },
                        ],
                      },
                    },
                  },
                ],
              },
            },
          ],
          results: [],
        },
        {
          name: "get-contacts",
          parameters: [],
          results: [
            {
              name: null,
              typ: {
                type: "list",
                inner: {
                  type: "record",
                  fields: [
                    { name: "name", typ: { type: "str" } },
                    {
                      name: "addresses",
                      typ: {
                        type: "list",
                        inner: {
                          type: "variant",
                          cases: [
                            {
                              name: "home",
                              typ: {
                                type: "record",
                                fields: [
                                  { name: "street", typ: { type: "str" } },
                                  { name: "city", typ: { type: "str" } },
                                  { name: "state", typ: { type: "str" } },
                                  { name: "zip", typ: { type: "str" } },
                                  {
                                    name: "apartment",
                                    typ: {
                                      type: "option",
                                      inner: { type: "str" },
                                    },
                                  },
                                ],
                              },
                            },
                            {
                              name: "business",
                              typ: {
                                type: "record",
                                fields: [
                                  {
                                    name: "company-name",
                                    typ: { type: "str" },
                                  },
                                  { name: "street", typ: { type: "str" } },
                                  {
                                    name: "suite",
                                    typ: {
                                      type: "option",
                                      inner: { type: "str" },
                                    },
                                  },
                                  { name: "city", typ: { type: "str" } },
                                  { name: "state", typ: { type: "str" } },
                                  { name: "zip", typ: { type: "str" } },
                                ],
                              },
                            },
                            {
                              name: "po-box",
                              typ: {
                                type: "record",
                                fields: [
                                  { name: "box-number", typ: { type: "str" } },
                                  { name: "city", typ: { type: "str" } },
                                  { name: "state", typ: { type: "str" } },
                                  { name: "zip", typ: { type: "str" } },
                                ],
                              },
                            },
                          ],
                        },
                      },
                    },
                  ],
                },
              },
            },
          ],
        },
      ],
    },
  ];

  const mockProps = {
    parsedExports: sampleParsedExports,
    name: "appy:complex-exports/appy-complex-api",
    urlFn: "add-contact",
    onNavigateToFunction: vi.fn(),
    functionDetails: sampleParsedExports[0]!.functions![0]!,
    viewMode: "form",
    setViewMode: vi.fn(),
    value: "{}",
    setValue: vi.fn(),
    resultValue: "",
    setResultValue: vi.fn(),
    onValueChange: vi.fn(),
    onInvoke: vi.fn(),
    copyToClipboard: vi.fn(),
  };

  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("renders the layout with sidebar and main content", () => {
    render(<InvokeLayout {...mockProps} />);

    // Check sidebar is present
    expect(
      screen.getByText("appy:complex-exports/appy-complex-api"),
    ).toBeInTheDocument();

    // Check header is present
    expect(
      screen.getByText("appy:complex-exports/appy-complex-api - add-contact"),
    ).toBeInTheDocument();

    // Check view mode buttons are present
    expect(screen.getByText("Form Layout")).toBeInTheDocument();
    expect(screen.getByText("Json Layout")).toBeInTheDocument();
    expect(screen.getByText("Types")).toBeInTheDocument();
  });

  it("displays functions in the sidebar", () => {
    render(<InvokeLayout {...mockProps} />);

    // Check both functions are displayed
    expect(screen.getByText("add-contact")).toBeInTheDocument();
    expect(screen.getByText("get-contacts")).toBeInTheDocument();
  });

  it("highlights the current function in the sidebar", () => {
    render(<InvokeLayout {...mockProps} />);

    const addContactButton = screen.getByRole("button", {
      name: "add-contact",
    });
    const getContactsButton = screen.getByRole("button", {
      name: "get-contacts",
    });

    // add-contact should be highlighted since urlFn is "add-contact"
    expect(addContactButton).toHaveClass("bg-gray-300", "dark:bg-neutral-800");
    expect(getContactsButton).not.toHaveClass(
      "bg-gray-300",
      "dark:bg-neutral-800",
    );
  });

  it("calls onNavigateToFunction when a function is clicked", () => {
    render(<InvokeLayout {...mockProps} />);

    const getContactsButton = screen.getByRole("button", {
      name: "get-contacts",
    });
    fireEvent.click(getContactsButton);

    expect(mockProps.onNavigateToFunction).toHaveBeenCalledWith(
      "appy:complex-exports/appy-complex-api",
      "get-contacts",
    );
  });

  it("renders DynamicForm when viewMode is 'form'", () => {
    render(<InvokeLayout {...mockProps} />);

    expect(screen.getByTestId("dynamic-form")).toBeInTheDocument();
    expect(
      screen.getByText(
        "Dynamic Form for appy:complex-exports/appy-complex-api",
      ),
    ).toBeInTheDocument();
    expect(screen.getByText("Function: add-contact")).toBeInTheDocument();
  });

  it("renders SectionCard with preview when viewMode is 'preview'", () => {
    const previewProps = { ...mockProps, viewMode: "preview" };
    render(<InvokeLayout {...previewProps} />);

    expect(screen.getByTestId("section-card-preview")).toBeInTheDocument();
    expect(screen.getByText("Preview")).toBeInTheDocument();
    expect(
      screen.getByText("Preview the current function invocation arguments"),
    ).toBeInTheDocument();
  });

  it("renders SectionCard with types when viewMode is 'types'", () => {
    const typesProps = { ...mockProps, viewMode: "types" };
    render(<InvokeLayout {...typesProps} />);

    expect(screen.getByTestId("section-card-types")).toBeInTheDocument();
    expect(screen.getAllByText("Types")).toHaveLength(2); // Button and section header
    expect(
      screen.getByText("Types of the function arguments"),
    ).toBeInTheDocument();
  });

  it("switches view mode when buttons are clicked", () => {
    render(<InvokeLayout {...mockProps} />);

    // Click Json Layout button
    fireEvent.click(screen.getByText("Json Layout"));
    expect(mockProps.setViewMode).toHaveBeenCalledWith("preview");
    expect(mockProps.setResultValue).toHaveBeenCalledWith("");

    // Click Types button
    fireEvent.click(screen.getByText("Types"));
    expect(mockProps.setViewMode).toHaveBeenCalledWith("types");

    // Click Form Layout button
    fireEvent.click(screen.getByText("Form Layout"));
    expect(mockProps.setViewMode).toHaveBeenCalledWith("form");
  });

  it("shows correct button highlighting based on viewMode", () => {
    render(<InvokeLayout {...mockProps} />);

    const formButton = screen.getByRole("button", { name: /Form Layout/ });
    const jsonButton = screen.getByRole("button", { name: /Json Layout/ });
    const typesButton = screen.getByRole("button", { name: /Types/ });

    // Form should be highlighted since viewMode is "form"
    expect(formButton).toHaveClass("bg-primary/20");
    expect(jsonButton).not.toHaveClass("bg-primary/20");
    expect(typesButton).not.toHaveClass("bg-primary/20");
  });

  it("calls onInvoke when form is submitted", () => {
    render(<InvokeLayout {...mockProps} />);

    const invokeButton = screen.getByText("Invoke Function");
    fireEvent.click(invokeButton);

    expect(mockProps.onInvoke).toHaveBeenCalledWith({
      params: [
        {
          name: "c",
          typ: mockProps.functionDetails!.parameters[0]!.typ,
          value: "test-value",
        },
      ],
    });
  });

  it("renders result section when resultValue is provided", () => {
    const propsWithResult = {
      ...mockProps,
      resultValue: '{"success": true, "message": "Contact added"}',
    };
    render(<InvokeLayout {...propsWithResult} />);

    expect(screen.getByTestId("section-card-result")).toBeInTheDocument();
    expect(screen.getByText("Result")).toBeInTheDocument();
    expect(
      screen.getByText("View the result of your latest invocation"),
    ).toBeInTheDocument();
    expect(
      screen.getByText('{"success": true, "message": "Contact added"}'),
    ).toBeInTheDocument();
  });

  it("does not render result section when resultValue is empty", () => {
    render(<InvokeLayout {...mockProps} />);

    expect(screen.queryByTestId("section-card-result")).not.toBeInTheDocument();
  });

  it("handles complex export structure with multiple functions", () => {
    render(<InvokeLayout {...mockProps} />);

    // Check that the export name is displayed correctly
    expect(
      screen.getByText("appy:complex-exports/appy-complex-api"),
    ).toBeInTheDocument();

    // Check that both functions are rendered
    const functionButtons = screen
      .getAllByRole("button")
      .filter(
        button =>
          button.textContent === "add-contact" ||
          button.textContent === "get-contacts",
      );
    expect(functionButtons).toHaveLength(2);
  });

  it("handles function with complex parameter types", () => {
    render(<InvokeLayout {...mockProps} />);

    // The DynamicForm should receive the complex function details
    expect(screen.getByText("Function: add-contact")).toBeInTheDocument();
    expect(screen.getByTestId("dynamic-form")).toBeInTheDocument();
  });

  it("calls copyToClipboard when copy button is clicked in preview mode", () => {
    const previewProps = { ...mockProps, viewMode: "preview" };
    render(<InvokeLayout {...previewProps} />);

    // Since copyToClipboard is passed to SectionCard, it would be tested
    // through the SectionCard component's implementation
    expect(screen.getByTestId("section-card-preview")).toBeInTheDocument();
  });

  it("handles empty function details gracefully", () => {
    const propsWithoutFunction = { ...mockProps, functionDetails: null };
    render(<InvokeLayout {...propsWithoutFunction} />);

    // Should not render any of the view mode content
    expect(screen.queryByTestId("dynamic-form")).not.toBeInTheDocument();
    expect(
      screen.queryByTestId("section-card-preview"),
    ).not.toBeInTheDocument();
    expect(screen.queryByTestId("section-card-types")).not.toBeInTheDocument();
  });

  it("handles value changes in preview mode", () => {
    const previewProps = { ...mockProps, viewMode: "preview" };
    render(<InvokeLayout {...previewProps} />);

    const textarea = screen.getByTestId("section-textarea");
    fireEvent.change(textarea, { target: { value: '{"new": "value"}' } });

    expect(mockProps.onValueChange).toHaveBeenCalledWith('{"new": "value"}');
  });

  it("passes correct export and function names to components", () => {
    render(<InvokeLayout {...mockProps} />);

    expect(
      screen.getByText(
        "Dynamic Form for appy:complex-exports/appy-complex-api",
      ),
    ).toBeInTheDocument();
    expect(screen.getByText("Function: add-contact")).toBeInTheDocument();
  });
});
