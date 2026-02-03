#!/usr/bin/env python3
"""
Configure Golem CLI MCP Server for Cursor
Cursor uses HTTP/SSE transport
"""

import json
import sys
import io
from pathlib import Path

# Fix Unicode encoding on Windows
if sys.platform == "win32":
    sys.stdout = io.TextIOWrapper(sys.stdout.buffer, encoding='utf-8', errors='replace')

def main():
    print("=" * 70)
    print("  Configuring Golem CLI MCP for Cursor")
    print("=" * 70)
    print()
    
    import os
    
    # Get Cursor config path
    appdata = os.getenv("APPDATA")
    if not appdata:
        print("✗ Error: APPDATA environment variable not set")
        return 1
    
    config_path = Path(appdata) / "Cursor" / "User" / "globalStorage" / "mcp.json"
    
    # Create config directory if it doesn't exist
    config_path.parent.mkdir(parents=True, exist_ok=True)
    
    # Read existing config or create new
    config = {}
    if config_path.exists():
        try:
            with open(config_path, 'r', encoding='utf-8') as f:
                config = json.load(f)
            print("✓ Found existing Cursor MCP config")
        except Exception as e:
            print(f"⚠ Warning: Could not read existing config: {e}")
            print("  Creating new config...")
    else:
        print("Creating new Cursor MCP config")
    
    # Ensure mcpServers exists
    if "mcpServers" not in config:
        config["mcpServers"] = {}
    
    # Add or update golem-cli configuration
    config["mcpServers"]["golem-cli"] = {
        "url": "http://127.0.0.1:3000/mcp"
    }
    
    # Write config
    try:
        with open(config_path, 'w', encoding='utf-8') as f:
            json.dump(config, f, indent=2, ensure_ascii=False)
        print(f"✓ Configuration written to: {config_path}")
        print()
        print("Configuration added:")
        print(json.dumps({"golem-cli": config["mcpServers"]["golem-cli"]}, indent=2))
    except Exception as e:
        print(f"✗ Error writing config: {e}")
        return 1
    
    print()
    print("=" * 70)
    print("  Configuration Complete!")
    print("=" * 70)
    print()
    print("Next steps:")
    print("1. Start the MCP server:")
    print("   golem-cli mcp-server start --host 127.0.0.1 --port 3000")
    print()
    print("2. Restart Cursor to load the configuration")
    print()
    
    return 0

if __name__ == "__main__":
    sys.exit(main())
