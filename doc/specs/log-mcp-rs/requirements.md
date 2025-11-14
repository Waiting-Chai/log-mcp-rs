# Requirements Document

## Introduction

log-mcp-rs 是一个基于 Rust 的高性能日志分析服务，通过 HTTP MCP 接口为 LLM Agent 提供智能日志诊断能力。系统支持按时间范围检索日志、自动抽取关键事件、生成诊断报告，采用零拷贝、并行扫描等技术实现秒级响应。

## Glossary

- **LogMCP**: 日志分析 MCP（Model Context Protocol）服务系统
- **Session**: 用户排查会话，包含挂载的日志文件、搜索历史和抽取的事实
- **FileManager**: 文件管理模块，负责目录扫描、文件映射、编码识别和解压
- **SearchEngine**: 搜索引擎模块，执行时间窗口过滤和关键词/正则匹配
- **FactExtractor**: 事实抽取器，从日志中提取结构化信息
- **TimelineBuilder**: 时间线构建器，合并跨文件事件
- **Reporter**: 报告生成器，输出 Markdown 或 JSON 格式报告
- **HTTPService**: HTTP 服务层，提供 RESTful API 接口
- **MemoryStore**: 记忆存储，保存排查过程中的结论和假设

## Requirements

### Requirement 1

**User Story:** 作为 LLM Agent，我希望能够创建独立的排查会话，以便并行处理多个日志分析任务

#### Acceptance Criteria

1. WHEN Agent 调用 /tools/start_session 接口，THE LogMCP SHALL 创建新的 Session 并返回唯一的 session_id
2. THE LogMCP SHALL 支持至少 20 个并发 Session 而不出现性能降级
3. WHEN Session 创建时指定时区参数，THE LogMCP SHALL 将该时区应用于该 Session 的所有时间解析操作
4. THE LogMCP SHALL 在 Session 中持久化会话状态到本地存储

### Requirement 2

**User Story:** 作为 LLM Agent，我希望能够挂载日志目录并自动识别文件格式，以便快速开始日志分析

#### Acceptance Criteria

1. WHEN Agent 调用 /tools/add_directory 接口并提供目录路径，THE FileManager SHALL 扫描目录并返回匹配的日志文件列表
2. THE FileManager SHALL 支持 glob 模式过滤（如 *.log, **/*.gz）
3. WHEN 调用 /tools/inspect_formats 接口，THE FileManager SHALL 自动识别每个文件的编码格式（UTF-8、GBK 等）
4. THE FileManager SHALL 自动识别时间戳格式和压缩类型（.gz）
5. THE FileManager SHALL 拒绝访问配置中 deny_patterns 指定的敏感文件

### Requirement 3

**User Story:** 作为 LLM Agent，我希望能够在指定时间范围内搜索日志，以便快速定位相关事件

#### Acceptance Criteria

1. WHEN Agent 调用 /tools/search 接口并指定 time_start 和 time_end，THE SearchEngine SHALL 仅返回该时间窗口内的日志行
2. THE SearchEngine SHALL 在 300MB 文件中完成关键词搜索，P50 延迟小于 1 秒
3. THE SearchEngine SHALL 在 1GB 文件中完成关键词加正则搜索，首次查询延迟小于 5 秒
4. WHEN 搜索结果超过 max_hits 配置限制，THE SearchEngine SHALL 截断结果并设置 truncated 标志为 true
5. THE SearchEngine SHALL 为每个命中结果返回上下文行（可配置行数）

### Requirement 4

**User Story:** 作为 LLM Agent，我希望能够使用多种匹配模式搜索日志，以便精确定位问题

#### Acceptance Criteria

1. WHEN Agent 在搜索查询中指定 must 关键词列表，THE SearchEngine SHALL 仅返回包含所有 must 关键词的日志行
2. WHEN Agent 在搜索查询中指定 any 关键词列表，THE SearchEngine SHALL 返回包含任意一个 any 关键词的日志行
3. WHEN Agent 在搜索查询中指定 none 关键词列表，THE SearchEngine SHALL 排除包含任何 none 关键词的日志行
4. WHEN Agent 在搜索查询中指定正则表达式，THE SearchEngine SHALL 对候选行执行正则匹配
5. THE SearchEngine SHALL 在正则匹配超过 regex_timeout_ms 配置时间时中止匹配

### Requirement 5

**User Story:** 作为 LLM Agent，我希望能够从日志中抽取结构化事实，以便进行深度分析

#### Acceptance Criteria

1. WHEN Agent 调用 /tools/extract_facts 接口并提供命中的日志 ID 列表，THE FactExtractor SHALL 应用预定义规则模板提取结构化字段
2. THE FactExtractor SHALL 支持提取常见字段（如 battery%、collision、task_id、error_code）
3. THE FactExtractor SHALL 在处理 200 个命中结果时完成抽取，延迟小于 500 毫秒
4. THE FactExtractor SHALL 返回 JSON 格式的结构化事实列表

### Requirement 6

**User Story:** 作为 LLM Agent，我希望能够记录排查过程中的结论和假设，以便在多轮对话中保持上下文

#### Acceptance Criteria

1. WHEN Agent 调用 /tools/remember 接口并提供键值对，THE MemoryStore SHALL 将该信息关联到当前 Session
2. WHEN Agent 调用 /tools/forget 接口并提供键，THE MemoryStore SHALL 从当前 Session 中删除该记忆
3. THE MemoryStore SHALL 在 Session 的整个生命周期内持久化记忆数据
4. THE MemoryStore SHALL 支持存储已排除假设和已确认结论

### Requirement 7

**User Story:** 作为 LLM Agent，我希望能够生成跨文件的时间线视图，以便理解事件的时序关系

#### Acceptance Criteria

1. WHEN Agent 调用 /tools/timeline 接口，THE TimelineBuilder SHALL 合并当前 Session 中所有搜索结果
2. THE TimelineBuilder SHALL 按时间戳升序排列事件
3. THE TimelineBuilder SHALL 为每个事件标注来源文件和行号
4. THE TimelineBuilder SHALL 处理跨文件的时间戳对齐

### Requirement 8

**User Story:** 作为 LLM Agent，我希望能够生成诊断报告，以便向用户呈现分析结果

#### Acceptance Criteria

1. WHEN Agent 调用 /tools/report 接口，THE Reporter SHALL 生成包含搜索结果、抽取事实和时间线的报告
2. THE Reporter SHALL 支持 Markdown 格式输出
3. THE Reporter SHALL 支持 JSON 格式输出
4. WHEN 配置 include_snippets 为 true，THE Reporter SHALL 在报告中包含日志片段

### Requirement 9

**User Story:** 作为系统管理员，我希望服务能够处理压缩日志文件，以便节省存储空间

#### Acceptance Criteria

1. WHEN FileManager 遇到 .gz 文件，THE FileManager SHALL 自动解压并读取内容
2. THE FileManager SHALL 在处理 .gz 文件时的耗时不超过纯文本文件的 2 倍
3. THE FileManager SHALL 支持流式解压以避免内存溢出

### Requirement 10

**User Story:** 作为系统管理员，我希望服务提供健康检查接口，以便监控服务状态

#### Acceptance Criteria

1. WHEN 外部系统调用 /health 接口，THE HTTPService SHALL 返回 HTTP 200 状态码和 JSON 响应 {"status":"healthy"}
2. THE HTTPService SHALL 在 100 毫秒内响应健康检查请求
3. THE HTTPService SHALL 在服务启动后立即可用于健康检查

### Requirement 11

**User Story:** 作为系统管理员，我希望能够通过配置文件控制服务行为，以便适应不同的部署环境

#### Acceptance Criteria

1. THE LogMCP SHALL 在启动时从 config.toml 文件加载配置
2. THE LogMCP SHALL 支持配置服务绑定地址和端口
3. THE LogMCP SHALL 支持配置允许访问的根目录列表（allow_roots）
4. THE LogMCP SHALL 支持配置文件大小限制、命中数限制和正则超时
5. THE LogMCP SHALL 支持配置是否启用临时索引及其阈值

### Requirement 12

**User Story:** 作为开发者，我希望服务能够提供可观测性指标，以便进行性能调优

#### Acceptance Criteria

1. THE LogMCP SHALL 使用 tracing 框架记录关键操作的日志
2. THE LogMCP SHALL 记录每个 HTTP 路由的响应时间
3. THE LogMCP SHALL 记录搜索命中数和内存占用
4. WHERE Prometheus 集成启用，THE LogMCP SHALL 暴露 /metrics 端点
