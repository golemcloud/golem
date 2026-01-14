#!/usr/bin/env python3
"""
Configure Golem CLI MCP Server for Cursor
Cursor uses HTTP/SSE transport
"""

import json
import os
import sys
from pathlib import Path

def find_golem_cli():
    """Find golem-cli executable"""
    script_dir = Path(__file__).parent
    
    possible_paths = [
        script_dir / "target" / "release" / "golem-cli.exe",
        script_dir / "target" / "debug" / "golem-cli.exe",
        "golem-cli.exe",
    ]
    
    # Check if golem-cli is in PATH
    import shutil
    path_exe = shutil.which("golem-cli.exe")
    if path_exe:
        possible_paths.append(path_exe)
    
    for path in possible_paths:
        if isinstance(path, str):
            if os.path.exists(path):
                return os.path.abspath(path)
        else:
            if path.exists():
                return str(path.resolve())
    
    return None

def main():
    print("=" * 50)
    print("  Configuring Golem CLI MCP for Cursor")
    print("=" * 50)
    print()
    
    # Find golem-cli
    golem_cli_path = find_golem_cli()
    if not golem_cli_path:
        print("ERROR: golem-cli.exe not found!", file=sys.stderr)
        print()
        print("Please build golem-cli first:")
        print("  cargo build --release --package golem-cli")
        print()
        print("Or ensure golem-cli is in your PATH")
        sys.exit(1)
    
    print(f"Found golem-cli: {golem_cli_path}")
    print()
    
    # Configuration path
    appdata = os.getenv("APPDATA")
    if not appdata:
        print("ERROR: APPDATA environment variable not set!", file=sys.stderr)
        sys.exit(1)
    
    config_path = Path(appdata) / "Cursor" / "User" / "globalStorage" / "mcp.json"
    config_dir = config_path.parent
    
    # Create config directory if it doesn't exist
    config_dir.mkdir(parents=True, exist_ok=True)
    if not config_dir.exists():
        print(f"Created config directory: {config_dir}")
    
    # Read existing config or create new
    config = {}
    if config_path.exists():
        try:
            with open(config_path, 'r', encoding='utf-8') as f:
                config = json.load(f)
            print("Found existing Cursor MCP config")
        except Exception as e:
            print(f"Error reading existing config, creating new... ({e})")
    else:
        print("Creating new Cursor MCP config")
    
    # Ensure mcpServers exists
    if "mcpServers" not in config:
        config["mcpServers"] = {}
    
    # Add golem-cli configuration (HTTP/SSE mode)
    config["mcpServers"]["golem-cli"] = {
        "url": "http://127.0.0.1:3000/mcp"
    }
    
    # Save configuration
    with open(config_path, 'w', encoding='utf-8') as f:
        json.dump(config, f, indent=2, ensure_ascii=False)
    
    print()
    print("=" * 50)
    print("  Configuration Complete!")
    print("=" * 50)
    print()
    print("Configuration saved to:")
    print(f"  {config_path}")
    print()
    print("Next steps:")
    print("1. Start the MCP server (HTTP/SSE mode):")
    print("   golem-cli mcp-server start")
    print()
    print("   Or with custom host/port:")
    print("   golem-cli mcp-server start --host 127.0.0.1 --port 3000")
    print()
    print("2. Restart Cursor to load the configuration")
    print()
    print("3. The MCP server will be available at:")
    print("   http://127.0.0.1:3000/mcp")
    print()
    print("Note: The server uses HTTP/SSE (Streamable HTTP) transport")

if __name__ == "__main__":
    main()
