# API Contract Examples

## POST /v1/tools/search (首次查询)

### Request
```json
{
  "session_id": "sess_abc123",
  "include_globs": ["**/*.log", "**/*.gz"],
  "exclude_globs": ["**/secret/*"],
  "time_start": "2025-11-13T20:00:00+08:00",
  "time_end": "2025-11-13T21:00:00+08:00",
  "mode": "text",
  "must": ["vehicle_001"],
  "any": ["battery", "collision", "任务", "交管"],
  "none": ["DEBUG"],
  "regex": "(?i)battery\\s*(\\d{1,3})\\s*%",
  "case_sensitive": false,
  "whole_word": false,
  "page_size": 100,
  "max_hits": 500,
  "hard_timeout_ms": 5000
}
```

### Response
```json
{
  "hits": [
    {
      "id": "hit_d7f9a1",
      "file": "/data/logs/traffic/app.log.1",
      "family_id": "traffic_app",
      "line_number": 183421,
      "byte_offset": 129384771,
      "timestamp": "2025-11-13T20:32:09.437Z",
      "content": "[TRAFFIC] vehicle_001 ... battery 15% ...",
      "context_before": ["..."],
      "context_after": ["..."],
      "truncated": false
    }
  ],
  "truncated": false,
  "cursor": "eyJzZXNzaW9uX2lkIjoic2Vzc19hYmMxMjMi...",
  "stats": {
    "files_scanned": 3,
    "bytes_scanned": 734003200,
    "compute_ms": 842,
    "io_mode_used": "mmap",
    "regex_cache_hits": 12,
    "ac_dict_size": 14,
    "candidate_lines": 1294,
    "skipped_none_hits": 317,
    "regex_timeouts": 0,
    "truncated_lines": 0
  }
}
```

## POST /v1/tools/search/continue (续查)

### Request
```json
{
  "cursor": "eyJzZXNzaW9uX2lkIjoic2Vzc19hYmMxMjMi...",
  "hard_timeout_ms": 5000
}
```

### Response
Same structure as /v1/tools/search

## Error Response Examples

### CURSOR_EXPIRED (400)
```json
{
  "error": "Cursor expired",
  "code": "CURSOR_EXPIRED",
  "retryable": true
}
```

### RATE_LIMITED (429)
```json
{
  "error": "Rate limited",
  "code": "RATE_LIMITED",
  "retryable": true,
  "retry_after_ms": 60000
}
```

### QUOTA_EXCEEDED (429)
```json
{
  "error": "Quota exceeded",
  "code": "QUOTA_EXCEEDED",
  "retryable": true
}
```

## Error Code Reference Table

| Code | HTTP Status | Retryable | Description |
|------|-------------|-----------|-------------|
| SESSION_NOT_FOUND | 404 | false | Session ID not found |
| FILE_DENIED | 403 | false | File access denied by security policy |
| BAD_TIME_RANGE | 400 | false | Invalid time range format |
| REGEX_TIMEOUT | 408 | true | Regex matching timed out |
| TOO_MANY_HITS | 206 | true | Hit limit reached, use cursor to continue |
| QUOTA_EXCEEDED | 429 | true | Session quota exceeded |
| CURSOR_EXPIRED | 400 | true | Cursor expired, need new search |
| RATE_LIMITED | 429 | true | Rate limit exceeded, retry after delay |
| INTERNAL | 500 | true | Internal server error |
