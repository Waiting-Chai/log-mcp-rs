# Implementation Plan

- [ ] 1. 初始化项目结构和核心配置
  - 创建 Cargo 项目，配置依赖（axum, tokio, serde, rayon, memmap2, aho-corasick, regex-automata, rusqlite, tracing, utoipa）
  - 实现完整 Config 结构和 TOML 配置解析（server, fs, limits, search, sessions, auth, security, clock_drift_policy）
  - 设置默认参数（max_hits: 500, regex_timeout_ms: 50, max_context_lines: 12, max_line_bytes: 1MB, page_size: 100, qps_per_key: 10, cursor_ttl: 600s）
  - 创建模块目录结构（axum_service, session_store, file_manager, search_engine, fact_extractor, timeline_builder, reporter）
  - 实现 LogMcpError 错误类型（包含 error_code() 和 is_retryable() 方法）
  - 实现完整错误码枚举（SESSION_NOT_FOUND, FILE_DENIED, BAD_TIME_RANGE, REGEX_TIMEOUT, TOO_MANY_HITS, QUOTA_EXCEEDED, CURSOR_EXPIRED, RATE_LIMITED, INTERNAL）
  - 创建错误码文档表（Code、HTTP Status、可恢复性、retry_after_ms）
  - _Requirements: 11.1, 11.2, 11.3, 11.4, 11.5_

- [x] 2. 实现 Session Manager 模块
  - [x] 2.1 创建 SQLite 数据库 Schema
    - 编写 SQL 建表语句（sessions, session_files, search_records, search_hits, memories, facts）
    - 添加性能索引（idx_search_records_session, idx_hits_record, idx_hits_ts）
    - 为 sessions 表添加 owner 字段（多租户预留）
    - 配置 SQLite WAL 模式（PRAGMA journal_mode=WAL; synchronous=NORMAL; busy_timeout=5000）
    - 实现数据库锁冲突指数退避重试（≤ 100ms × 5 次）
    - 实现数据库初始化和迁移逻辑
    - _Requirements: 1.4_
  
  - [x] 2.2 实现 SessionManager 核心功能
    - 实现 create_session 方法生成唯一 session_id
    - 实现 get_session 方法从数据库加载会话
    - 实现 add_files 方法添加文件到会话
    - 实现 add_search_record 方法记录搜索历史
    - 实现 set_memory 和 remove_memory 方法管理记忆
    - 实现 per-session bytes_scanned_quota 检查和 QUOTA_EXCEEDED 错误
    - _Requirements: 1.1, 1.3, 6.1, 6.2, 6.3, 6.4_
  
  - [x]* 2.3 编写 Session Manager 单元测试
    - 测试会话创建和查询
    - 测试并发会话访问
    - 测试记忆存储和删除
    - 测试配额超限和 TTL 清理
    - _Requirements: 1.2_
  
  - [x] 2.4 实现会话生命周期管理
    - 实现后台定时任务清理过期会话（session_ttl 默认 7 天）
    - 清理会话关联的临时文件和索引
    - 实现磁盘配额检查（max_session_bytes 默认 5GB）
    - _Requirements: 1.4_

- [x] 3. 实现 File Manager 模块
  - [x] 3.1 实现目录扫描和 glob 匹配
    - 编写 scan_directory 方法支持 glob 模式
    - 实现 allow_roots 和 deny_patterns 安全检查
    - 实现路径规范化和父路径前缀判断（防止路径逃逸）
    - Windows 下检测 REPARSE_POINT 并拒绝
    - Unix 下使用 O_RDONLY | O_NOFOLLOW 打开文件
    - _Requirements: 2.1, 2.2, 2.5_
  
  - [x] 3.2 实现编码检测和格式识别
    - 集成 chardetng 检测文件编码（UTF-8, GBK, GB18030）
    - 显式检测 BOM（UTF-8-BOM、UTF-16LE/BE），返回 needs_convert 标志
    - 实现时间戳格式自动识别（正则匹配常见格式，返回 timestamp_examples 样例和可信度）
    - 识别压缩类型（.gz 扩展名）
    - 支持多种换行符（\n, \r\n, \r）和超长行处理（max_line_bytes 限制，返回 truncated_lines 计数）
    - 检测 NDJSON/JSON Lines 格式，设置 mode: "json"
    - _Requirements: 2.3, 2.4_
  
  - [x] 3.3 实现文件映射和解压
    - 使用 memmap2 实现 map_file 方法
    - 实现 io_mode 配置（mmap/buffered/auto），auto 模式检测 NFS 并回退到 buffered
    - 使用 flate2 实现 .gz 文件流式解压（串行 MVP，记录 scanned_bytes_uncompressed 和 cpu_ms）
    - 实现 MappedFile 结构提供统一接口
    - 保持字节级扫描（bstr），仅在返回文本时转码
    - 在统计中标注 io_mode_used
    - _Requirements: 9.1, 9.3_
  
  - [x]* 3.4 编写 File Manager 单元测试
    - 测试 glob 匹配和安全检查
    - 测试编码检测准确性
    - 测试 .gz 文件解压性能
    - _Requirements: 9.2_

- [ ] 4. 实现 Search Engine 核心算法
  - [ ] 4.1 实现时间窗索引和过滤
    - 编写时间戳解析逻辑（支持多种格式，统一转换为 UTC）
    - 实现稀疏时间戳索引（每 1000 行采样）
    - 实现二分查找定位时间窗口起止偏移
    - 应用 clock_drift_policy 容忍度（默认 ±3s）
    - _Requirements: 3.1_
  
  - [ ] 4.2 实现 Aho-Corasick 多关键词匹配
    - 构建 Aho-Corasick 自动机（must, any, none）
    - 支持 case_sensitive 和 whole_word 选项（whole_word 使用 regex-automata 的 \b 边界确认，避免后处理）
    - 实现单次扫描匹配所有关键词
    - 实现 none 关键词短路逻辑
    - 统计 ac_dict_size 和 skipped_none_hits
    - _Requirements: 4.1, 4.2, 4.3_
  
  - [ ] 4.3 实现正则匹配和超时控制
    - 使用 regex-automata 编译正则为 DFA
    - 实现正则预编译缓存（LRU）和命中率指标
    - 实现正则匹配超时机制（regex_timeout_ms）
    - 仅对候选行执行正则匹配
    - 记录 regex_timeout_total 指标用于熔断
    - _Requirements: 4.4, 4.5_
  
  - [ ] 4.4 实现并行扫描和短路机制
    - 使用 rayon 并行处理多文件
    - 实现大文件分块并行扫描
    - 实现 max_hits 和 max_scanned_bytes 双重短路机制
    - 支持 CancellationToken 实现可取消搜索
    - _Requirements: 3.4_
  
  - [ ] 4.5 实现搜索结果分页和游标
    - 实现 SearchCursor 结构（session_id, file, byte_off, ts_floor, pattern_cache_id, page_size, issued_at）
    - 实现 cursor 的 base64url 编码和解码
    - 实现 cursor 过期检查（cursor_ttl 默认 600s），过期返回 CURSOR_EXPIRED
    - 支持分页查询（基于 cursor 继续搜索）
    - 提取命中行的前后 N 行（max_context_lines）
    - 实现超长行截断（max_line_bytes），确保 UTF-8 边界，标记 truncated: true
    - 生成 SearchHit 结构包含上下文、family_id、byte_offset、truncated 标志
    - _Requirements: 3.5_
  
  - [ ] 4.6 实现结构化日志（JSON）搜索
    - 检测 mode: "json" 时解析 NDJSON/JSON Lines
    - 支持 json_path_must/any/none 和 time_field 配置
    - 实现 time_field 容错（不存在或类型异常时回退到文本时间戳探测）
    - 在 inspect_formats 返回 json_time_field_confidence
    - 对 JSON 字段执行关键词和正则匹配
    - _Requirements: 3.1, 4.1_
  
  - [ ] 4.7 实现文件轮转聚合
    - 识别文件族（app.log, app.log.1, app.log.2.gz）并分配 family_id
    - 按时间排序聚合同族文件（基于 mtime 和序号）
    - 在 search 中默认跨同族顺序扫描（可配置关闭）
    - _Requirements: 2.1_
  
  - [ ]* 4.8 编写 Search Engine 性能测试
    - 测试 300MB 文件关键词搜索 P50 < 1s
    - 测试 1GB 文件关键词+正则首次 < 5s
    - 测试并行扫描加速效果
    - 测试文件轮转聚合正确性
    - _Requirements: 3.2, 3.3_

- [ ] 5. 实现 Fact Extractor 模块
  - [ ] 5.1 实现规则模板加载
    - 解析 facts.d/*.toml 规则文件
    - 编译规则中的正则表达式
    - 创建 ExtractionRule 结构
    - _Requirements: 5.1_
  
  - [ ] 5.2 实现字段提取逻辑
    - 对每个 SearchHit 应用所有规则
    - 提取正则捕获组到字段
    - 生成 Fact 结构关联 source_hit_id
    - _Requirements: 5.2, 5.4_
  
  - [ ] 5.3 实现敏感信息脱敏
    - 从配置加载 redact.d/*.toml 脱敏规则（邮箱、手机号、token、cookie）
    - 在 extract_facts 和 report 中应用脱敏（可选开关）
    - 使用占位符替换敏感信息（如 [EMAIL_REDACTED]）
    - _Requirements: 5.4_
  
  - [ ]* 5.4 编写 Fact Extractor 性能测试
    - 测试 200 命中结果抽取 < 500ms
    - 测试规则匹配准确性
    - 测试脱敏功能正确性
    - _Requirements: 5.3_

- [ ] 6. 实现 Timeline Builder 模块
  - [ ] 6.1 实现事件收集和排序
    - 从 Session 收集所有 SearchHit
    - 解析每个 Hit 的时间戳
    - 按时间戳升序排序
    - _Requirements: 7.1, 7.2_
  
  - [ ] 6.2 实现事件关联和标注
    - 为每个事件标注来源文件和行号
    - 关联已抽取的 Fact 到对应事件
    - 应用 clock_drift_policy 处理时间戳对齐
    - 生成 TimelineEvent 结构
    - _Requirements: 7.3, 7.4_
  
  - [ ]* 6.3 编写 Timeline Builder 单元测试
    - 测试跨文件事件合并
    - 测试时间戳对齐
    - 测试事实关联

- [ ] 7. 实现 Reporter 模块
  - [ ] 7.1 实现 Markdown 报告生成
    - 创建报告模板（概要、时间线、关键事实、已知结论）
    - 实现时间线表格格式化
    - 实现日志片段包含逻辑（include_snippets）
    - _Requirements: 8.1, 8.2, 8.4_
  
  - [ ] 7.2 实现 JSON 报告生成
    - 序列化 Report 结构为 JSON
    - 确保 JSON 格式符合规范
    - _Requirements: 8.3_
  
  - [ ]* 7.3 编写 Reporter 单元测试
    - 测试 Markdown 格式正确性
    - 测试 JSON 序列化
    - 测试多语言支持

- [ ] 8. 实现 HTTP Service 层
  - [ ] 8.1 创建 Axum 路由和中间件
    - 定义所有 /v1/tools/* 路由（API 版本化）
    - 实现 API Key / Bearer Token 认证 middleware
    - 实现 IP 白名单检查 middleware
    - 实现 RateLimit middleware（每 API key QPS 限流，令牌桶算法，返回 retry_after_ms）
    - 实现 content_length_limit（per-route，默认 10MB）和响应压缩（gzip/br）
    - 实现 Timeout middleware（支持 hard_timeout_ms）
    - 实现 Tracing middleware
    - 实现 CORS middleware（默认关闭，开启时仅允许明确来源，生产禁止 *）
    - _Requirements: 10.1_
  
  - [ ] 8.2 实现会话管理接口
    - 实现 POST /v1/tools/start_session 处理器
    - 实现 POST /v1/tools/add_directory 处理器（支持文件轮转识别和 family_id）
    - 实现 POST /v1/tools/inspect_formats 处理器（返回编码、时间格式、timestamp_examples、io_mode_used、sequence）
    - _Requirements: 1.1, 2.1, 2.3_
  
  - [ ] 8.3 实现搜索和分析接口
    - 实现 POST /v1/tools/search 处理器（支持分页 cursor、include_globs、exclude_globs、mode: text/json）
    - 实现 POST /v1/tools/search/continue 处理器（基于 cursor 续查）
    - 实现 POST /v1/tools/validate_regex 处理器（dry-run 验证正则，限制样本数和超时）
    - 实现 POST /v1/tools/extract_facts 处理器（支持可选脱敏 redact_patterns）
    - 实现 POST /v1/tools/remember 处理器
    - 实现 POST /v1/tools/forget 处理器
    - _Requirements: 3.1, 5.1, 6.1, 6.2_
  
  - [ ] 8.4 实现时间线和报告接口
    - 实现 POST /v1/tools/timeline 处理器（支持 tz_out 本地化输出）
    - 实现 POST /v1/tools/report 处理器（支持脱敏和 include_snippets）
    - _Requirements: 7.1, 8.1_
  
  - [ ] 8.5 实现健康检查和 OpenAPI
    - 实现 GET /health 处理器返回 {"status":"healthy"}
    - 确保响应时间 < 100ms
    - 使用 utoipa 生成 OpenAPI 规范，导出 GET /v1/openapi.json
    - 在 OpenAPI 中声明 bearerAuth 安全方案（HTTP bearer）
    - 为所有 /v1/tools/* 路由标注 security: [{"bearerAuth": []}]
    - 标注所有 timestamp 字段为 RFC3339 格式
    - _Requirements: 10.1, 10.2, 10.3_
  
  - [ ] 8.6 实现优雅退出
    - 捕获 SIGTERM 信号
    - 拒绝新请求，等待进行中搜索完成
    - 在 graceful_shutdown_timeout 到达后取消未完成任务
    - 写入"未完成标记"避免脏状态
    - _Requirements: 10.1_
  
  - [ ]* 8.7 编写 HTTP Service 集成测试
    - 测试端到端流程（创建会话 → 搜索 → 报告）
    - 测试错误处理和 HTTP 状态码
    - 测试并发请求
    - 测试优雅退出流程

- [ ] 9. 实现可观测性和监控
  - [ ] 9.1 集成 Tracing 框架
    - 配置 tracing_subscriber
    - 为关键操作添加 span 和 event
    - 输出日志到 stdout
    - _Requirements: 12.1_
  
  - [ ] 9.2 实现性能指标收集
    - 记录每个路由的响应时间（request_latency_bucket）
    - 记录搜索命中数（search_hits_total）和扫描字节数（bytes_scanned_total）
    - 记录正则超时次数（regex_timeout_total）和编译缓存命中率（regex_cache_hits）
    - 记录 AC 字典大小（ac_dict_size）和 mmap 缺页中断（mmap_faults_total，可选）
    - 实现熔断机制（正则连续 N 次超时或扫描字节超限时拒绝同一 API key 的重型查询 1 分钟）
    - 在错误响应中返回 retry_after_ms
    - _Requirements: 12.2, 12.3_
  
  - [ ]* 9.3 集成 Prometheus 导出器
    - 实现 GET /metrics 端点
    - 导出关键指标（请求计数、延迟、错误率）
    - _Requirements: 12.4_

- [ ] 10. 创建部署配置和文档
  - [ ] 10.1 编写 Dockerfile
    - 实现多阶段构建
    - 使用 distroless 基础镜像
    - 配置健康检查
    - _Requirements: 10.1_
  
  - [ ] 10.2 编写 docker-compose.yml
    - 配置服务端口映射
    - 配置卷挂载（config, logs, sessions）
    - 配置环境变量和健康检查
    - _Requirements: 11.1_
  
  - [ ] 10.3 创建配置文件模板
    - 编写 config.toml 示例
    - 包含所有配置项的注释说明
    - _Requirements: 11.2, 11.3, 11.4, 11.5_
  
  - [ ] 10.4 编写 README 文档
    - 项目介绍和功能说明
    - 快速开始指南（Docker 部署）
    - API 接口文档和 curl 示例（基于 OpenAPI）
    - 错误码表（Code、HTTP Status、可恢复性）
    - 性能基线测试结果
    - 安全实践文档（脱敏、白名单、配额、鉴权）
    - 容量规划和风险清单（NFS、编码、ReDoS、磁盘膨胀）
    - _Requirements: All_
  
  - [ ] 10.5 创建配置文件模板
    - 编写生产环境 config.toml（严格安全配置）
    - 编写开发环境 config-dev.toml（宽松配置）
    - 包含所有配置项的注释说明
    - _Requirements: 11.2, 11.3, 11.4, 11.5_
  
  - [ ]* 10.6 创建示例日志文件和规则
    - 生成测试用日志文件（不同大小、编码、CRLF/LF）
    - 编写示例 Fact 抽取规则（facts.d/*.toml）
    - 编写示例脱敏规则（redact.d/*.toml）
    - _Requirements: 5.1_

- [ ] 11. 性能优化和压力测试
  - [ ] 11.1 替换内存分配器
    - 集成 mimalloc 或 jemalloc
    - 对比性能提升
    - _Requirements: 3.2, 3.3_
  
  - [ ] 11.2 执行性能基准测试
    - 运行 300MB 文件搜索测试（目标 P50 < 1s）
    - 运行 1GB 文件搜索测试（目标首次 < 5s）
    - 运行 .gz 文件测试（目标 < 2× 纯文本）
    - 运行事实抽取测试（目标 200 命中 < 500ms）
    - _Requirements: 3.2, 3.3, 5.3, 9.2_
  
  - [ ] 11.3 执行并发压力测试
    - 测试 20 并发会话吞吐
    - 验证无性能降级
    - _Requirements: 1.2_
  
  - [ ]* 11.4 性能调优和优化
    - 根据 Profiling 结果优化热点代码
    - 调整缓存策略和并行度
    - 生成性能测试报告
    - _Requirements: 3.2, 3.3_

- [ ] 12. 跨平台 CI 和发布
  - [ ] 12.1 配置 GitHub Actions CI
    - 配置多平台矩阵（ubuntu-latest, macos-latest, windows-latest）
    - Windows 专项测试 CRLF 和路径处理
    - 运行单元测试和集成测试
    - _Requirements: All_
  
  - [ ] 12.2 配置 Release 构建
    - 构建多平台二进制（Linux x86_64, macOS arm64/x86_64, Windows x86_64）
    - 构建 musl 静态链接版本（Linux）
    - 自动发布到 GitHub Releases
    - _Requirements: All_
