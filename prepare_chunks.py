
import math
import sys

def read_file(path):
    with open(path, 'r') as f:
        return [line.strip() for line in f]

lines = read_file('/Users/maweilong/Desktop/github/log-mcp-rs/src.tar.gz.b64')
total_lines = len(lines)
chunk_size = 5
num_chunks = math.ceil(total_lines / chunk_size)

start_chunk = 0
end_chunk = num_chunks

if len(sys.argv) > 1:
    start_chunk = int(sys.argv[1])
if len(sys.argv) > 2:
    end_chunk = int(sys.argv[2])

for i in range(start_chunk, end_chunk):
    start = i * chunk_size
    end = start + chunk_size
    chunk_lines = lines[start:end]
    content = "\\n".join(chunk_lines)
    filename = f"part_{i:03d}"
    print(f"echo \"{content}\" > /home/log-mcp-rs/{filename}")
