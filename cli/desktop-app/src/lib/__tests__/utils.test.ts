import {
  cn,
  formatRelativeTime,
  sanitizeInput,
  formatTimestampInDateTimeFormat,
} from "../utils";

describe("utils", () => {
  describe("cn", () => {
    it("merges class names correctly", () => {
      const result = cn("base-class", "additional-class");
      expect(result).toBe("base-class additional-class");
    });

    it("handles conditional classes", () => {
      const result = cn(
        "base-class",
        true && "conditional-class",
        false && "hidden-class",
      );
      expect(result).toBe("base-class conditional-class");
    });

    it("handles undefined and null values", () => {
      const result = cn("base-class", undefined, null, "valid-class");
      expect(result).toBe("base-class valid-class");
    });

    it("handles empty strings", () => {
      const result = cn("base-class", "", "valid-class");
      expect(result).toBe("base-class valid-class");
    });

    it("handles arrays of classes", () => {
      const result = cn(["base-class", "array-class"], "additional-class");
      expect(result).toBe("base-class array-class additional-class");
    });

    it("handles objects with boolean values", () => {
      const result = cn("base-class", {
        "conditional-true": true,
        "conditional-false": false,
        "another-true": true,
      });
      expect(result).toBe("base-class conditional-true another-true");
    });

    it("handles complex combinations", () => {
      const result = cn(
        "base-class",
        ["array-class"],
        {
          "conditional-true": true,
          "conditional-false": false,
        },
        "final-class",
      );
      expect(result).toBe(
        "base-class array-class conditional-true final-class",
      );
    });

    it("returns empty string for no arguments", () => {
      const result = cn();
      expect(result).toBe("");
    });
  });

  describe("formatRelativeTime", () => {
    beforeEach(() => {
      // Mock Date.now() to return a fixed timestamp
      vi.useFakeTimers();
      vi.setSystemTime(new Date("2024-01-01T12:00:00Z"));
    });

    afterEach(() => {
      vi.useRealTimers();
    });

    it('returns "just now" for current time', () => {
      const now = new Date();
      const result = formatRelativeTime(now);
      expect(result).toBe("just now");
    });

    it("formats seconds ago correctly", () => {
      const thirtySecondsAgo = new Date(Date.now() - 30 * 1000);
      const result = formatRelativeTime(thirtySecondsAgo);
      expect(result).toBe("30 seconds ago");
    });

    it("formats single second ago correctly", () => {
      const oneSecondAgo = new Date(Date.now() - 1000);
      const result = formatRelativeTime(oneSecondAgo);
      expect(result).toBe("1 second ago");
    });

    it("formats minutes ago correctly", () => {
      const fiveMinutesAgo = new Date(Date.now() - 5 * 60 * 1000);
      const result = formatRelativeTime(fiveMinutesAgo);
      expect(result).toBe("5 minutes ago");
    });

    it("formats single minute ago correctly", () => {
      const oneMinuteAgo = new Date(Date.now() - 60 * 1000);
      const result = formatRelativeTime(oneMinuteAgo);
      expect(result).toBe("1 minute ago");
    });

    it("formats hours ago correctly", () => {
      const threeHoursAgo = new Date(Date.now() - 3 * 60 * 60 * 1000);
      const result = formatRelativeTime(threeHoursAgo);
      expect(result).toBe("3 hours ago");
    });

    it("formats days ago correctly", () => {
      const twoDaysAgo = new Date(Date.now() - 2 * 24 * 60 * 60 * 1000);
      const result = formatRelativeTime(twoDaysAgo);
      expect(result).toBe("2 days ago");
    });

    it("formats weeks ago correctly", () => {
      const twoWeeksAgo = new Date(Date.now() - 2 * 7 * 24 * 60 * 60 * 1000);
      const result = formatRelativeTime(twoWeeksAgo);
      expect(result).toBe("2 weeks ago");
    });

    it("formats months ago correctly", () => {
      const twoMonthsAgo = new Date(Date.now() - 2 * 30 * 24 * 60 * 60 * 1000);
      const result = formatRelativeTime(twoMonthsAgo);
      expect(result).toBe("2 months ago");
    });

    it("formats years ago correctly", () => {
      const twoYearsAgo = new Date(Date.now() - 2 * 365 * 24 * 60 * 60 * 1000);
      const result = formatRelativeTime(twoYearsAgo);
      expect(result).toBe("2 years ago");
    });

    it("handles string input", () => {
      const dateString = new Date(Date.now() - 60 * 1000).toISOString();
      const result = formatRelativeTime(dateString);
      expect(result).toBe("1 minute ago");
    });

    it("handles number input", () => {
      const timestamp = Date.now() - 60 * 1000;
      const result = formatRelativeTime(timestamp);
      expect(result).toBe("1 minute ago");
    });
  });

  describe("sanitizeInput", () => {
    it("replaces curly quotes with straight quotes", () => {
      const input = '"Hello" "World"';
      const result = sanitizeInput(input);
      expect(result).toBe('"Hello" "World"');
    });

    it("replaces Unicode quotes with straight quotes", () => {
      const input = "\u201cHello\u201d \u201cWorld\u201d";
      const result = sanitizeInput(input);
      expect(result).toBe('"Hello" "World"');
    });

    it("replaces curly apostrophes with straight apostrophes", () => {
      const input = "don't can't";
      const result = sanitizeInput(input);
      expect(result).toBe("don't can't");
    });

    it("handles mixed quotes and apostrophes", () => {
      const input = "\"Don't say 'hello'\"";
      const result = sanitizeInput(input);
      expect(result).toBe("\"Don't say 'hello'\"");
    });

    it("returns unchanged string when no special characters", () => {
      const input = "Hello World";
      const result = sanitizeInput(input);
      expect(result).toBe("Hello World");
    });

    it("handles empty string", () => {
      const result = sanitizeInput("");
      expect(result).toBe("");
    });
  });

  describe("formatTimestampInDateTimeFormat", () => {
    it("formats timestamp in MM/DD/YYYY HH:MM:SS format", () => {
      const timestamp = "2024-01-15T10:30:45.000Z";
      const result = formatTimestampInDateTimeFormat(timestamp);

      // The result will depend on the local timezone, so we'll check the format
      expect(result).toMatch(/^\d{2}\/\d{2}\/\d{4} \d{2}:\d{2}:\d{2}$/);
    });

    it("handles different timestamp formats", () => {
      const timestamp = "2024-12-25T23:59:59Z";
      const result = formatTimestampInDateTimeFormat(timestamp);

      expect(result).toMatch(/^\d{2}\/\d{2}\/\d{4} \d{2}:\d{2}:\d{2}$/);
    });

    it("pads single digits with zeros", () => {
      const timestamp = "2024-01-05T09:05:05.000Z";
      const result = formatTimestampInDateTimeFormat(timestamp);

      // Should have leading zeros for month, day, hour, minute, second
      expect(result).toMatch(/^\d{2}\/\d{2}\/\d{4} \d{2}:\d{2}:\d{2}$/);
    });
  });
});
