import { render, screen, fireEvent } from "@testing-library/react";
import { describe, it, expect, vi } from "vitest";
import { RecursiveParameterInput } from "../RecursiveParameterInput";
import { Typ } from "@/types/component";

// Mock lucide-react icons
vi.mock("lucide-react", () => ({
  MinusCircle: () => <div data-testid="minus-circle">-</div>,
  PlusCircle: () => <div data-testid="plus-circle">+</div>,
  ChevronDown: () => <div data-testid="chevron-down">v</div>,
  ChevronUp: () => <div data-testid="chevron-up">^</div>,
  Check: () => <div data-testid="check">✓</div>,
}));

describe("RecursiveParameterInput", () => {
  const sampleAddressTypeDef: Typ = {
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
                typ: { type: "option", inner: { type: "str" } },
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
                typ: { type: "option", inner: { type: "str" } },
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
  };

  const mockOnChange = vi.fn();

  beforeEach(() => {
    mockOnChange.mockClear();
  });

  it("renders the component with name and type badge", () => {
    render(
      <RecursiveParameterInput
        name="addresses"
        typeDef={sampleAddressTypeDef}
        value={[]}
        onChange={mockOnChange}
        path="c"
      />,
    );

    // Check that the name is displayed
    expect(screen.getByText("addresses")).toBeInTheDocument();

    // Check that the type badge is displayed
    expect(screen.getByText("list")).toBeInTheDocument();
  });

  it("shows 'No items added' message when list is empty", () => {
    render(
      <RecursiveParameterInput
        name="addresses"
        typeDef={sampleAddressTypeDef}
        value={[]}
        onChange={mockOnChange}
        path="c"
      />,
    );

    expect(screen.getByText("No items added")).toBeInTheDocument();
  });

  it("shows 'Add Item' button for list type", () => {
    render(
      <RecursiveParameterInput
        name="addresses"
        typeDef={sampleAddressTypeDef}
        value={[]}
        onChange={mockOnChange}
        path="c"
      />,
    );

    const addButton = screen.getByRole("button", { name: /add item/i });
    expect(addButton).toBeInTheDocument();
  });

  it("adds a new item when 'Add Item' button is clicked", () => {
    render(
      <RecursiveParameterInput
        name="addresses"
        typeDef={sampleAddressTypeDef}
        value={[]}
        onChange={mockOnChange}
        path="c"
      />,
    );

    const addButton = screen.getByRole("button", { name: /add item/i });
    fireEvent.click(addButton);

    expect(mockOnChange).toHaveBeenCalledTimes(1);
    expect(mockOnChange).toHaveBeenCalledWith(
      "c.addresses",
      expect.arrayContaining([
        expect.objectContaining({
          home: expect.objectContaining({
            apartment: null,
            street: expect.any(String),
          }),
        }),
      ]),
    );
  });

  it("shows variant selection and form fields after adding an item", () => {
    const valueWithOneItem = [
      { home: { street: "", city: "", state: "", zip: "", apartment: null } },
    ];

    render(
      <RecursiveParameterInput
        name="addresses"
        typeDef={sampleAddressTypeDef}
        value={valueWithOneItem}
        onChange={mockOnChange}
        path="c"
      />,
    );

    // Should show variant selector with "home" selected
    expect(screen.getByRole("combobox")).toBeInTheDocument();
    expect(screen.getByRole("combobox")).toHaveTextContent("home");

    // Should show the form fields for home address
    expect(screen.getByPlaceholderText("Enter street...")).toBeInTheDocument();
    expect(screen.getByPlaceholderText("Enter city...")).toBeInTheDocument();
    expect(screen.getByPlaceholderText("Enter state...")).toBeInTheDocument();
    expect(screen.getByPlaceholderText("Enter zip...")).toBeInTheDocument();
  });

  it("shows all required home address fields when variant is selected", () => {
    const valueWithOneItem = [
      { home: { street: "", city: "", state: "", zip: "", apartment: null } },
    ];

    render(
      <RecursiveParameterInput
        name="addresses"
        typeDef={sampleAddressTypeDef}
        value={valueWithOneItem}
        onChange={mockOnChange}
        path="c"
      />,
    );

    // Check that all home address fields are present
    expect(screen.getByText("street")).toBeInTheDocument();
    expect(screen.getByText("city")).toBeInTheDocument();
    expect(screen.getByText("state")).toBeInTheDocument();
    expect(screen.getByText("zip")).toBeInTheDocument();
    expect(screen.getByText("apartment")).toBeInTheDocument();

    // Check input fields are rendered
    expect(screen.getByPlaceholderText("Enter street...")).toBeInTheDocument();
    expect(screen.getByPlaceholderText("Enter city...")).toBeInTheDocument();
    expect(screen.getByPlaceholderText("Enter state...")).toBeInTheDocument();
    expect(screen.getByPlaceholderText("Enter zip...")).toBeInTheDocument();
  });

  it("allows changing variant type from home to business", () => {
    const valueWithOneItem = [
      { home: { street: "", city: "", state: "", zip: "", apartment: null } },
    ];

    render(
      <RecursiveParameterInput
        name="addresses"
        typeDef={sampleAddressTypeDef}
        value={valueWithOneItem}
        onChange={mockOnChange}
        path="c"
      />,
    );

    // Find and click the select to open it
    const selectTrigger = screen.getByRole("combobox");
    fireEvent.click(selectTrigger);

    // Select "business" option
    const businessOption = screen.getByRole("option", { name: "business" });
    fireEvent.click(businessOption);

    // Check that onChange was called with business variant
    expect(mockOnChange).toHaveBeenCalledTimes(1);
    const call = mockOnChange.mock.calls[0];
    expect(call![0]).toBe("c.addresses");
    expect(call![1]).toHaveLength(1);
    expect(call![1][0]).toHaveProperty("business");
    expect(call![1][0].business).toHaveProperty("company-name");
    expect(call![1][0].business).toHaveProperty("suite", null);
  });

  it("shows business address fields when business variant is selected", () => {
    const valueWithBusinessItem = [
      {
        business: {
          "company-name": "",
          street: "",
          suite: null,
          city: "",
          state: "",
          zip: "",
        },
      },
    ];

    render(
      <RecursiveParameterInput
        name="addresses"
        typeDef={sampleAddressTypeDef}
        value={valueWithBusinessItem}
        onChange={mockOnChange}
        path="c"
      />,
    );

    // Check business-specific fields
    expect(screen.getByText("company-name")).toBeInTheDocument();
    expect(screen.getByText("suite")).toBeInTheDocument();
    expect(
      screen.getByPlaceholderText("Enter company-name..."),
    ).toBeInTheDocument();
    // Suite is optional, so its input is not shown unless enabled
    expect(screen.getByText("Optional value")).toBeInTheDocument();
  });

  it("allows removing items from the list", () => {
    const valueWithOneItem = [
      { home: { street: "", city: "", state: "", zip: "", apartment: null } },
    ];

    render(
      <RecursiveParameterInput
        name="addresses"
        typeDef={sampleAddressTypeDef}
        value={valueWithOneItem}
        onChange={mockOnChange}
        path="c"
      />,
    );

    // Find and click the remove button (minus icon)
    const removeButton = screen.getByTestId("minus-circle")
      .parentElement as HTMLElement;
    fireEvent.click(removeButton);

    // Check that onChange was called with empty array
    expect(mockOnChange).toHaveBeenCalledWith("c.addresses", []);
  });

  it("handles input changes for string fields", () => {
    const valueWithOneItem = [
      { home: { street: "", city: "", state: "", zip: "", apartment: null } },
    ];

    render(
      <RecursiveParameterInput
        name="addresses"
        typeDef={sampleAddressTypeDef}
        value={valueWithOneItem}
        onChange={mockOnChange}
        path="c"
      />,
    );

    // Find street input and type in it
    const streetInput = screen.getByPlaceholderText("Enter street...");
    fireEvent.change(streetInput, { target: { value: "123 Main St" } });

    // Check that onChange was called with updated street value
    expect(mockOnChange).toHaveBeenCalledWith("c.addresses", [
      {
        home: {
          street: "123 Main St",
          city: "",
          state: "",
          zip: "",
          apartment: null,
        },
      },
    ]);
  });

  it("handles optional fields correctly", () => {
    const valueWithOneItem = [
      { home: { street: "", city: "", state: "", zip: "", apartment: null } },
    ];

    render(
      <RecursiveParameterInput
        name="addresses"
        typeDef={sampleAddressTypeDef}
        value={valueWithOneItem}
        onChange={mockOnChange}
        path="c"
      />,
    );

    // Find the optional checkbox for apartment
    const apartmentSection = screen.getByText("apartment").parentElement;
    const optionalCheckbox = apartmentSection?.querySelector(
      'input[type="checkbox"]',
    ) as HTMLInputElement;

    expect(optionalCheckbox).toBeInTheDocument();
    expect(optionalCheckbox.checked).toBe(false);

    // Click the checkbox to enable the optional field
    fireEvent.click(optionalCheckbox);

    // Should call onChange with the apartment field enabled
    expect(mockOnChange).toHaveBeenCalledWith("c.addresses", [
      { home: { street: "", city: "", state: "", zip: "", apartment: "" } },
    ]);
  });

  it("supports multiple address items in the list", () => {
    const valueWithMultipleItems = [
      {
        home: {
          street: "123 Main St",
          city: "City1",
          state: "State1",
          zip: "12345",
          apartment: null,
        },
      },
      {
        business: {
          "company-name": "ACME Corp",
          street: "456 Oak Ave",
          suite: null,
          city: "City2",
          state: "State2",
          zip: "67890",
        },
      },
    ];

    render(
      <RecursiveParameterInput
        name="addresses"
        typeDef={sampleAddressTypeDef}
        value={valueWithMultipleItems}
        onChange={mockOnChange}
        path="c"
      />,
    );

    // Should show both items
    expect(screen.getByDisplayValue("123 Main St")).toBeInTheDocument();
    expect(screen.getByDisplayValue("ACME Corp")).toBeInTheDocument();

    // Should have two remove buttons
    const removeButtons = screen.getAllByTestId("minus-circle");
    expect(removeButtons).toHaveLength(2);
  });
});
