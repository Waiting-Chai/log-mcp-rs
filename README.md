# Log Search MCP Server

Rust 实现的高性能日志搜索 MCP (Model Context Protocol) 服务器，支持 SSE (Server-Sent Events) 和 Stdio 两种模式。

它允许 AI Agent (如 Trae, Claude Desktop, Bisheng 等) 实时读取、搜索和分析服务器上的日志文件。支持多行日志解析、逻辑组合搜索、时间范围过滤以及并发文件扫描。

![License](https://img.shields.io/badge/license-MIT-blue.svg)
![Rust](https://img.shields.io/badge/rust-1.75%2B-orange.svg)

## 🌟 核心特性

- **双模式支持**：
  - **SSE 模式**：通过 HTTP 提供服务，支持远程访问，适用于 Bisheng 等 web 架构的 Agent。
  - **Stdio 模式**：通过标准输入输出交互，适用于本地 Trae、Claude Desktop 等客户端。
- **高效搜索**：
  - 支持逻辑组合 (AND/OR/NOT)、正则表达式、时间范围过滤。
  - 自动识别多行日志（如 Java 堆栈跟踪）。
  - 并发扫描多个日志文件。
- **配置热更新**：修改配置文件后自动重载，无需重启服务。
- **部署友好**：
  - 提供 Docker 和 Docker Compose 一键部署方案。
  - 针对国内网络环境优化了 Docker 构建过程（使用阿里云源和 rsproxy）。
- **文件处理**：自动处理 Gzip 压缩文件，支持多种编码检测。

## 🚀 快速开始 (Docker Compose 推荐)

这是最简单的部署方式，适合在服务器上长期运行。

### 1. 获取代码

```bash
git clone https://github.com/your-repo/log-mcp-rs.git
cd log-mcp-rs
```

### 2. 配置

项目根目录已经包含 `docker-compose.yml` 和 `config.example.yaml`。

你可以根据需要修改 `docker-compose.yml` 中的卷映射，将主机上的日志目录映射到容器内：

```yaml
volumes:
  - ./config.yaml:/app/config.yaml  # 配置文件
  - /var/log:/var/log               # 系统日志目录
  - /home/logs:/home/logs           # 应用日志目录 (根据实际情况修改)
```

确保 `config.yaml` 存在（可以复制示例）：

```bash
cp config.example.yaml config.yaml
```

修改 `config.yaml` 以匹配你的需求，特别是 `log_file_paths`：

```yaml
server:
  mode: http       # 使用 http 模式以支持 SSE
  port: 3000
  host: "0.0.0.0"

log_sources:
  log_file_paths:
    - "/var/log/syslog"
    - "/home/logs/app.log"
```

### 3. 启动服务

```bash
docker compose up -d --build
```

服务将在 `http://localhost:3000` 启动。

## 🛠️ 本地开发与测试

如果你想在本地运行而不使用 Docker：

### 前置要求

- Rust 1.75+
- Cargo

### 运行

```bash
# 1. 复制配置文件
cp config.example.yaml config.yaml

# 2. 运行 (默认读取 config.yaml)
cargo run --release -- config.yaml
```

## 🔌 集成指南

### 1. 集成到 Bisheng (SSE 模式)

在 Bisheng 的 MCP 服务器配置中，添加以下 JSON 配置：

```json
{
  "mcpServers": {
    "log-search": {
      "type": "sse",
      "name": "日志搜索服务",
      "description": "提供服务器日志的实时搜索和查询功能。支持按关键词、正则表达式、时间范围过滤日志。适用于系统故障排查、错误日志定位。",
      "url": "http://<你的服务器IP>:3000/sse"
    }
  }
}
```

### 2. 集成到 Trae / Claude Desktop (Stdio 模式)

如果作为本地工具运行，或者通过 SSH 隧道连接，可以使用 Stdio 模式。

修改 `config.yaml`：
```yaml
server:
  mode: stdio
```

在 Trae/Claude 的配置中添加：

```json
{
  "mcpServers": {
    "log-search": {
      "command": "/path/to/log-search-mcp",
      "args": [
        "/path/to/config.yaml"
      ]
    }
  }
}
```

## ⚙️ 配置文件说明 (`config.yaml`)

```yaml
server:
  mode: http         # 运行模式: http, stdio, 或 both
  port: 3000         # HTTP 端口 (仅 http/both 模式有效)
  host: "0.0.0.0"    # 监听地址

log_parser:
  line_start_regex: '^\d{4}-\d{2}-\d{2}'  # 用于识别多行日志的起始行正则
  default_timestamp_regex: '\d{4}-\d{2}-\d{2}[T ]\d{2}:\d{2}:\d{2}' # 时间戳提取正则

search:
  default_page_size: 20
  max_page_size: 200
  default_timeout_ms: 5000
  max_concurrent_files: 4

log_sources:
  log_file_paths:    # 待扫描的日志文件绝对路径
    - "/var/log/syslog"
```

## 📡 API 接口 (SSE 模式)

- **GET /sse**: 建立 SSE 连接，接收服务端事件。
- **POST /message**: 发送 JSON-RPC 请求 (如 `list_tools`, `call_tool`)。

## 📝 开发日志

- **2024-12-12**: 
  - 新增 SSE Server 支持，兼容标准 MCP 协议。
  - 添加 `docker-compose` 支持，优化部署流程。
  - 优化 Dockerfile 国内构建速度。
  - 修复多行日志解析问题。
