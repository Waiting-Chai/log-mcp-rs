import subprocess
import json
import time

mcp_server_path = "/Users/maweilong/Desktop/github/log-mcp-rs/target/release/log-search-mcp"
config_path = "/Users/maweilong/Desktop/github/log-mcp-rs/mcp_config.yaml"

process = subprocess.Popen(
    [mcp_server_path, config_path],
    stdin=subprocess.PIPE,
    stdout=subprocess.PIPE,
    stderr=subprocess.PIPE,
    text=True
)

# 1. Initialize
init_req = {
    "jsonrpc": "2.0",
    "id": 1,
    "method": "initialize",
    "params": {
        "protocolVersion": "2024-11-05",
        "capabilities": {},
        "clientInfo": {"name": "test-client", "version": "1.0"}
    }
}

process.stdin.write(json.dumps(init_req) + "\n")
process.stdin.flush()
print("Sent initialize")

init_resp = process.stdout.readline()
print(f"Init Response: {init_resp}")

# 2. Initialized Notification
process.stdin.write(json.dumps({
    "jsonrpc": "2.0",
    "method": "notifications/initialized"
}) + "\n")
process.stdin.flush()
print("Sent initialized notification")

# Read potential response (though my code might not send one for notification if id is null, but let's see)
# In my code: if req.id.is_null() { continue; } so no response.

# 3. List Tools
list_tools_req = {
    "jsonrpc": "2.0",
    "id": 2,
    "method": "tools/list",
    "params": {}
}

process.stdin.write(json.dumps(list_tools_req) + "\n")
process.stdin.flush()
print("Sent tools/list")

tools_resp = process.stdout.readline()
print(f"Tools Response: {tools_resp}")

process.terminate()
