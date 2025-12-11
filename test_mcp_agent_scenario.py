import sys
import json
import subprocess
import time
from datetime import datetime

# 配置部分
MCP_BINARY = "./target/release/log-search-mcp"
CONFIG_FILE = "./mcp_config.yaml"
# 使用绝对路径指向 binary 和 config
BINARY_PATH = "/Users/maweilong/Desktop/github/log-mcp-rs/target/release/log-search-mcp"
CONFIG_PATH = "/Users/maweilong/Desktop/github/log-mcp-rs/mcp_config.yaml"
# 模拟 Trae 的执行环境：CWD 为用户主目录
CWD = "/Users/maweilong"

def run_mcp_request(request_json):
    """
    运行 MCP 二进制文件，发送 JSON-RPC 请求并获取响应
    """
    try:
        cmd = [BINARY_PATH, CONFIG_PATH]
        # print(f"DEBUG: Starting MCP server with command: {cmd} in CWD: {CWD}")
        
        process = subprocess.Popen(
            cmd,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=sys.stderr,
            text=True,
            bufsize=1,
            cwd=CWD
        )
        
        if "jsonrpc" in request_json:
             json_rpc_req = request_json
        else:
             json_rpc_req = {
                "jsonrpc": "2.0",
                "id": "1",
                "method": "search_logs",
                "params": request_json.get("arguments", request_json)
            }
        
        input_str = json.dumps(json_rpc_req) + "\n"
        
        # 发送请求
        stdout, stderr = process.communicate(input=input_str, timeout=30)
        
        if stderr:
            print(f"MCP Stderr: {stderr}", file=sys.stderr)
            
        return stdout
    except Exception as e:
        print(f"Error running MCP: {e}", file=sys.stderr)
        return None

def parse_mcp_response(response):
    if not response:
        return None, "No response"
    
    try:
        resp_json = json.loads(response)
        if "error" in resp_json:
            return None, resp_json['error']
            
        result_data = resp_json.get("result", {})
        if "content" in result_data:
             try:
                text = result_data["content"][0]["text"]
                if result_data.get("isError"):
                    return None, text
                hits = json.loads(text).get("hits", [])
                return hits, None
             except Exception as e:
                return None, f"Failed to parse content text: {e}"
        else:
             hits = result_data.get("hits", [])
             return hits, None
    except Exception as e:
        return None, f"JSON parse error: {e}"

def perform_search(step_name, must_keywords, time_start, time_end):
    print(f"\n--- {step_name} ---")
    print(f"Keywords: {must_keywords}")
    print(f"Time: {time_start} to {time_end}")
    
    args = {
        "include_content": True,
        "log_start_pattern": "^\\d{4}-\\d{2}-\\d{2} \\d{2}:\\d{2}:\\d{2}\\.\\d{3}",
        "logical_query": {
            "any": [],
            "must": must_keywords,
            "none": []
        },
        "page": 1,
        "page_size": 100,
        "scan_config": {
            "include_globs": ["**/*.log", "**/*.log.gz"],
            # 使用空 root_path 依赖全局配置，或指向特定目录
            "root_path": "/Users/maweilong/fsdownload"
        },
        "time_filter": {
            "after": time_start,
            "before": time_end
        }
    }
    
    req = {
        "jsonrpc": "2.0",
        "id": "1",
        "method": "tools/call",
        "params": {
            "name": "search_logs",
            "arguments": args
        }
    }
    
    resp = run_mcp_request(req)
    hits, error = parse_mcp_response(resp)
    
    if error:
        print(f"❌ Error: {error}")
        return []
    
    print(f"✅ Found {len(hits)} logs")
    return hits

def analyze_troubleshooting():
    print("=== 开始 MCP Agent 自动化排查 ===")
    
    # 时间范围: 2025-11-08 13:59:30 至 14:00:30
    START_TIME = "2025-11-08 13:59:30"
    END_TIME = "2025-11-08 14:00:30"
    VEHICLE_ID = "sim_0015"
    
    # 步骤 1: 确认车辆是否进入交管循环
    hits1 = perform_search(
        "步骤 1: 确认车辆是否进入交管循环", 
        ["traffic#beforeDoPreOccupy", VEHICLE_ID],
        START_TIME, END_TIME
    )
    if not hits1:
        print("🔴 未找到资源申请日志。可能原因：车辆无任务、状态异常或 Controller 未下发请求。")
    else:
        print(f"✅ 车辆已发起资源申请 (Found {len(hits1)} logs)")
        print(f"   示例: {hits1[0]['content'].strip()[:100]}...")

    # 步骤 2: 检查锁资源是否被抢占
    hits2 = perform_search(
        "步骤 2: 检查锁资源是否被抢占",
        ["traffic#lockPoint", VEHICLE_ID],
        START_TIME, END_TIME
    )
    
    lock_failed = False
    if not hits2:
        print("⚠️ 未找到锁点尝试日志")
    else:
        print(f"✅ 找到锁点尝试日志 (Found {len(hits2)} logs)")
        for hit in hits2:
            content = hit["content"]
            if "failedResult" in content and "[]" not in content:
                print(f"� 发现锁点失败: {content.strip()[:150]}...")
                lock_failed = True
                if "OCCUPIED" in content:
                     print("   -> 原因: 资源被占用 (OCCUPIED)")
                elif "DEADLOCK" in content:
                     print("   -> 原因: 死锁 (DEADLOCK)")
                break
        if not lock_failed:
             print("✅ 未发现显式的锁点失败记录 (可能是成功锁定)")

    # 步骤 3: 检查是否存在死锁或系统错误
    # 这里我们演示使用 'any' 查询
    print(f"\n--- 步骤 3: 检查是否存在死锁或系统错误 ---")
    args3 = {
        "include_content": True,
        "logical_query": {
            "any": ["traffic#doingLockError", "LockFailedReason.DEADLOCK"],
            "must": [],
            "none": []
        },
        "scan_config": {"root_path": "/Users/maweilong/fsdownload"},
        "time_filter": {"after": START_TIME, "before": END_TIME}
    }
    req3 = {
        "jsonrpc": "2.0", "id": "1", "method": "tools/call",
        "params": {"name": "search_logs", "arguments": args3}
    }
    resp3 = run_mcp_request(req3)
    hits3, err3 = parse_mcp_response(resp3)
    
    if err3:
        print(f"❌ Error: {err3}")
    elif hits3:
        print(f"🔴 警告: 发现系统错误或死锁日志 ({len(hits3)} 条)")
        print(f"   示例: {hits3[0]['content'].strip()[:100]}...")
    else:
        print("✅ 未发现死锁或系统错误日志")

    # 步骤 4: 检查资源释放情况
    hits4 = perform_search(
        "步骤 4: 检查资源释放情况",
        ["traffic#unlockRequestPoints", VEHICLE_ID],
        START_TIME, END_TIME
    )
    if hits4:
        print(f"✅ 车辆已执行资源释放 ({len(hits4)} 条)")
    else:
        print("⚠️ 未找到资源释放日志 (如果之前锁点失败，这是正常的)")

if __name__ == "__main__":
    analyze_troubleshooting()
