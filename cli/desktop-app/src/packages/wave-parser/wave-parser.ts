/**
 * WAVE (WebAssembly Value Encoding) Parser
 * Translates WAVE format strings into plain JavaScript objects
 * Based on the WAVE specification and JSON-WAVE mapping
 */

export function parseWave(input: string): any {
  const parser = new WaveParser(input);
  return parser.parse();
}

class WaveParser {
  private input: string;
  private pos: number = 0;

  constructor(input: string) {
    this.input = input.trim();
  }

  parse(): any {
    this.skipWhitespace();
    if (this.pos >= this.input.length) {
      throw new Error("Empty input");
    }

    const result = this.parseValue();
    this.skipWhitespace();

    if (this.pos < this.input.length) {
      throw new Error(`Unexpected characters at position ${this.pos}: ${this.input.slice(this.pos)}`);
    }

    return result;
  }

  private parseValue(): any {
    this.skipWhitespace();

    const char = this.peek();

    // Handle different value types
    if (char === '"') {
      return this.parseString();
    }
    if (char === "'") {
      return this.parseChar();
    }
    if (char === "[") {
      return this.parseList();
    }
    if (char === "(") {
      return this.parseTuple();
    }
    if (char === "{") {
      const nextChar = this.peekAhead(1);
      // Distinguish between records and flags
      if (this.isFlags()) {
        return this.parseFlags();
      } else {
        return this.parseRecord();
      }
    }

    // Handle keywords and identifiers
    if (this.isAlpha(char) || char === "-" || char === "+") {
      return this.parseKeywordOrNumber();
    }

    // Handle numbers
    if (this.isDigit(char)) {
      return this.parseNumber();
    }

    throw new Error(`Unexpected character '${char}' at position ${this.pos}`);
  }

  private parseString(): string {
    this.expect('"');
    let result = "";

    while (this.pos < this.input.length && this.peek() !== '"') {
      if (this.peek() === "\\") {
        this.advance();
        const escaped = this.advance();
        switch (escaped) {
          case "n":
            result += "\n";
            break;
          case "t":
            result += "\t";
            break;
          case "r":
            result += "\r";
            break;
          case "\\":
            result += "\\";
            break;
          case '"':
            result += '"';
            break;
          case "'":
            result += "'";
            break;
          default:
            result += escaped;
            break;
        }
      } else {
        result += this.advance();
      }
    }

    this.expect('"');
    return result;
  }

  private parseChar(): number {
    this.expect("'");
    let char: string;

    if (this.peek() === "\\") {
      this.advance();
      const escaped = this.advance();
      switch (escaped) {
        case "n":
          char = "\n";
          break;
        case "t":
          char = "\t";
          break;
        case "r":
          char = "\r";
          break;
        case "\\":
          char = "\\";
          break;
        case "'":
          char = "'";
          break;
        case '"':
          char = '"';
          break;
        default:
          char = escaped;
          break;
      }
    } else {
      // Handle multi-byte UTF-8 characters
      const start = this.pos;
      // Find the end of the character (before the closing quote)
      while (this.pos < this.input.length && this.peek() !== "'") {
        this.advance();
      }
      char = this.input.slice(start, this.pos);
    }

    this.expect("'");
    return char.codePointAt(0) || 0;
  }

  private parseList(): any[] {
    this.expect("[");
    const result: any[] = [];

    this.skipWhitespace();
    if (this.peek() === "]") {
      this.advance();
      return result;
    }

    while (true) {
      result.push(this.parseValue());
      this.skipWhitespace();

      if (this.peek() === "]") {
        this.advance();
        break;
      }

      this.expect(",");
    }

    return result;
  }

  private parseTuple(): any[] {
    this.expect("(");
    const result: any[] = [];

    this.skipWhitespace();
    if (this.peek() === ")") {
      this.advance();
      return result;
    }

    while (true) {
      result.push(this.parseValue());
      this.skipWhitespace();

      if (this.peek() === ")") {
        this.advance();
        break;
      }

      this.expect(",");
      this.skipWhitespace();

      // Handle trailing comma in single-element tuple
      if (this.peek() === ")") {
        this.advance();
        break;
      }
    }

    return result;
  }

  private parseRecord(): Record<string, any> {
    this.expect("{");
    const result: Record<string, any> = {};

    this.skipWhitespace();
    if (this.peek() === "}") {
      this.advance();
      return result;
    }

    while (true) {
      this.skipWhitespace();
      const key = this.parseIdentifier();
      this.skipWhitespace();
      this.expect(":");
      this.skipWhitespace();
      const value = this.parseValue();

      result[key] = value;

      this.skipWhitespace();
      if (this.peek() === "}") {
        this.advance();
        break;
      }

      this.expect(",");
    }

    return result;
  }

  private parseFlags(): string[] {
    this.expect("{");
    const result: string[] = [];

    this.skipWhitespace();
    if (this.peek() === "}") {
      this.advance();
      return result;
    }

    while (true) {
      this.skipWhitespace();
      const flag = this.parseIdentifier();
      result.push(flag);

      this.skipWhitespace();
      if (this.peek() === "}") {
        this.advance();
        break;
      }

      this.expect(",");
    }

    return result;
  }

  private parseKeywordOrNumber(): any {
    const start = this.pos;

    // Handle negative numbers
    if (this.peek() === "-" && this.isDigit(this.peekAhead(1))) {
      return this.parseNumber();
    }

    // Handle positive numbers with explicit +
    if (this.peek() === "+" && this.isDigit(this.peekAhead(1))) {
      return this.parseNumber();
    }

    // Parse identifier
    while (
      this.pos < this.input.length &&
      (this.isAlphaNum(this.peek()) || this.peek() === "-" || this.peek() === "_")
    ) {
      this.advance();
    }

    const identifier = this.input.slice(start, this.pos);

    // Validate identifier
    if (identifier === "") {
      throw new Error(`Invalid identifier at position ${start}`);
    }

    // Check for function calls (variants, options, results)
    this.skipWhitespace();
    if (this.peek() === "(") {
      return this.parseFunctionCall(identifier);
    }

    // Handle special keywords
    switch (identifier) {
      case "true":
        return true;
      case "false":
        return false;
      case "none":
        return null;
      case "inf":
        return Infinity;
      case "-inf":
        return -Infinity;
      case "nan":
        return NaN;
      default:
        // For unknown identifiers, check if they're valid keywords
        if (/^[a-zA-Z][a-zA-Z0-9_-]*$/.test(identifier)) {
          return identifier; // Treat as enum value
        }
        throw new Error(`Invalid identifier: ${identifier}`);
    }
  }

  private parseFunctionCall(functionName: string): any {
    this.expect("(");

    // Handle empty parameter list
    this.skipWhitespace();
    if (this.peek() === ")") {
      this.advance();

      switch (functionName) {
        case "some":
          return null; // some() is equivalent to none
        case "ok":
          return { ok: null };
        case "err":
          return { err: null };
        default:
          return { [functionName]: null };
      }
    }

    const value = this.parseValue();
    this.skipWhitespace();
    this.expect(")");

    switch (functionName) {
      case "some":
        return value;
      case "ok":
        return { ok: value };
      case "err":
        return { err: value };
      default:
        return { [functionName]: value };
    }
  }

  private parseNumber(): number {
    const start = this.pos;

    // Handle sign
    if (this.peek() === "-" || this.peek() === "+") {
      this.advance();
    }

    // Handle integer part
    if (!this.isDigit(this.peek())) {
      throw new Error(`Expected digit at position ${this.pos}`);
    }

    while (this.pos < this.input.length && this.isDigit(this.peek())) {
      this.advance();
    }

    // Handle decimal part
    if (this.peek() === ".") {
      this.advance();
      while (this.pos < this.input.length && this.isDigit(this.peek())) {
        this.advance();
      }
    }

    // Handle scientific notation
    if (this.peek() === "e" || this.peek() === "E") {
      this.advance();
      if (this.peek() === "+" || this.peek() === "-") {
        this.advance();
      }
      while (this.pos < this.input.length && this.isDigit(this.peek())) {
        this.advance();
      }
    }

    // Check for invalid characters immediately following the number
    if (this.pos < this.input.length && this.isAlpha(this.peek())) {
      throw new Error(`Invalid number format: numbers cannot be followed by letters at position ${this.pos}`);
    }

    const numberStr = this.input.slice(start, this.pos);
    const result = Number(numberStr);

    if (isNaN(result)) {
      throw new Error(`Invalid number: ${numberStr}`);
    }

    return result;
  }

  private parseIdentifier(): string {
    const start = this.pos;

    if (!this.isAlpha(this.peek()) && this.peek() !== "_") {
      throw new Error(`Expected identifier at position ${this.pos}`);
    }

    while (
      this.pos < this.input.length &&
      (this.isAlphaNum(this.peek()) || this.peek() === "_" || this.peek() === "-")
    ) {
      this.advance();
    }

    return this.input.slice(start, this.pos);
  }

  private isFlags(): boolean {
    // Look ahead to determine if this is a flags structure
    const saved = this.pos;
    this.advance(); // skip '{'
    this.skipWhitespace();

    if (this.peek() === "}") {
      this.pos = saved;
      // Empty braces could be either empty record or empty flags
      // We need more context, but let's default to empty record for now
      return false;
    }

    // Check if next token is an identifier without colon
    const start = this.pos;
    while (
      this.pos < this.input.length &&
      (this.isAlphaNum(this.peek()) || this.peek() === "_" || this.peek() === "-")
    ) {
      this.advance();
    }

    if (this.pos === start) {
      this.pos = saved;
      return false;
    }

    this.skipWhitespace();
    const isFlags = this.peek() !== ":";
    this.pos = saved;
    return isFlags;
  }

  private isVariantContext(): boolean {
    // Simple heuristic: if we're not in a record context, treat as variant
    // This is a simplified approach - in practice, you'd need type information
    return false;
  }

  private peek(): string {
    return this.pos < this.input.length ? this.input[this.pos]! : "";
  }

  private peekAhead(offset: number): string {
    const newPos = this.pos + offset;
    return newPos < this.input.length ? this.input[newPos]! : "";
  }

  private advance(): string {
    if (this.pos >= this.input.length) {
      throw new Error("Unexpected end of input");
    }
    return this.input[this.pos++]!;
  }

  private expect(expected: string): void {
    if (this.pos >= this.input.length) {
      throw new Error(`Expected '${expected}' but reached end of input`);
    }

    if (this.input.slice(this.pos, this.pos + expected.length) !== expected) {
      throw new Error(
        `Expected '${expected}' at position ${this.pos}, got '${this.input.slice(
          this.pos,
          this.pos + expected.length
        )}'`
      );
    }

    this.pos += expected.length;
  }

  private skipWhitespace(): void {
    while (this.pos < this.input.length && /\s/.test(this.input[this.pos]!)) {
      this.pos++;
    }
  }

  private isAlpha(char: string): boolean {
    return /[a-zA-Z]/.test(char);
  }

  private isDigit(char: string): boolean {
    return /[0-9]/.test(char);
  }

  private isAlphaNum(char: string): boolean {
    return this.isAlpha(char) || this.isDigit(char);
  }
}
