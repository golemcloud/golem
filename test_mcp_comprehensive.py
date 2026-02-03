#!/usr/bin/env python3
"""Comprehensive MCP server testing with tool calls"""

import json
import sys
import io
import time
from list_mcp_tools import McpClient

# Fix Unicode encoding on Windows
if sys.platform == "win32":
    sys.stdout = io.TextIOWrapper(sys.stdout.buffer, encoding='utf-8', errors='replace')

def test_tool_call(client: McpClient, tool_name: str):
    """Test calling an MCP tool"""
    try:
        response = client.request("tools/call", {
            "name": tool_name,
            "arguments": {}
        })
        
        if "error" in response:
            return {"success": False, "error": response["error"]}
        elif "result" in response:
            return {"success": True, "result": response["result"]}
        else:
            return {"success": False, "error": "Unexpected response format"}
    except Exception as e:
        return {"success": False, "error": str(e)}

def main():
    print("=" * 70)
    print("  Comprehensive MCP Server Testing")
    print("=" * 70)
    print()
    
    try:
        client = McpClient("127.0.0.1", 3000)
        
        print("1. Connecting to MCP server...")
        client.connect()
        print("   ✓ Connected\n")
        
        print("2. Initializing MCP session...")
        init_result = client.initialize()
        if "result" in init_result:
            server_info = init_result["result"].get("serverInfo", {})
            print(f"   ✓ Initialized")
            print(f"     Server: {server_info.get('name', 'unknown')} v{server_info.get('version', 'unknown')}")
            print(f"     Protocol: {init_result['result'].get('protocolVersion', 'unknown')}")
        else:
            print(f"   ✗ Failed: {init_result}")
            return 1
        print()
        
        print("3. Listing available tools...")
        tools = client.list_tools()
        print(f"   ✓ Found {len(tools)} tool(s)\n")
        
        if not tools:
            print("   ✗ No tools found - server might not be configured correctly")
            return 1
        
        print("4. Testing tool calls...")
        print()
        all_success = True
        
        for i, tool in enumerate(tools, 1):
            tool_name = tool.get('name')
            tool_desc = tool.get('description', 'No description')
            
            print(f"   Tool {i}: {tool_name}")
            print(f"   Description: {tool_desc}")
            
            result = test_tool_call(client, tool_name)
            
            if result["success"]:
                print(f"   ✓ Call successful")
                # Show summary of result
                result_data = result.get("result", {})
                if "content" in result_data:
                    content = result_data["content"]
                    if content and len(content) > 0:
                        text_content = content[0].get("text", "")
                        try:
                            parsed = json.loads(text_content)
                            # Show summary
                            if isinstance(parsed, dict):
                                keys = list(parsed.keys())
                                print(f"   Result keys: {', '.join(keys[:5])}")
                            else:
                                print(f"   Result type: {type(parsed).__name__}")
                        except:
                            preview = text_content[:100]
                            print(f"   Result preview: {preview}...")
            else:
                print(f"   ✗ Call failed: {result.get('error', 'Unknown error')}")
                all_success = False
            print()
        
        print("5. Testing additional MCP endpoints...")
        print()
        
        # Test resources/list
        print("   Testing resources/list...")
        try:
            resources_response = client.request("resources/list", {})
            if "result" in resources_response:
                resources = resources_response["result"].get("resources", [])
                print(f"   ✓ Found {len(resources)} resource(s)")
            elif "error" in resources_response:
                error_code = resources_response["error"].get("code", -1)
                if error_code == -32601:  # Method not found
                    print(f"   - Resources not supported (method_not_found)")
                else:
                    print(f"   ✗ Error: {resources_response['error']}")
            else:
                print(f"   - No resources endpoint")
        except Exception as e:
            print(f"   - Resources not available: {e}")
        print()
        
        # Test prompts/list
        print("   Testing prompts/list...")
        try:
            prompts_response = client.request("prompts/list", {})
            if "result" in prompts_response:
                prompts = prompts_response["result"].get("prompts", [])
                print(f"   ✓ Found {len(prompts)} prompt(s)")
            elif "error" in prompts_response:
                error_code = prompts_response["error"].get("code", -1)
                if error_code == -32601:  # Method not found
                    print(f"   - Prompts not supported (method_not_found)")
                else:
                    print(f"   ✗ Error: {prompts_response['error']}")
            else:
                print(f"   - No prompts endpoint")
        except Exception as e:
            print(f"   - Prompts not available: {e}")
        print()
        
        client.close()
        
        print("=" * 70)
        print("  Test Summary")
        print("=" * 70)
        print()
        print(f"✓ Tools discovered: {len(tools)}")
        print(f"✓ Tool calls: {'All successful' if all_success else 'Some failed'}")
        print(f"✓ Server: Operational")
        print()
        
        if all_success and len(tools) >= 3:
            print("Server is working correctly!")
        
        return 0 if all_success else 1
        
    except Exception as e:
        print(f"✗ Fatal error: {e}")
        import traceback
        traceback.print_exc()
        return 1

if __name__ == "__main__":
    sys.exit(main())
