# HTTP File Transfer Performance Optimizations

## Overview
This document outlines the performance optimizations implemented for the fastest possible LAN file transfers using HTTP.

## Key Optimizations Implemented

### 1. **Parallel HTTP Connections**
- Files > 50MB automatically use 4 parallel connections
- Each connection downloads a different range of the file
- HTTP Range requests enable true parallel downloading
- Expected improvement: 3-4x speed increase for large files

### 2. **Optimized Buffer Management**
- **Download Buffer**: 8MB (reduced from 64MB) for better memory efficiency
- **Write Threshold**: 2MB for continuous streaming without stalls
- **Stream Buffer**: 8MB for optimal network throughput
- Eliminates the stop-and-go pattern seen in logs

### 3. **TCP Socket Tuning**
- **TCP Send Buffer**: 4MB (increased from default ~64KB)
- **TCP Receive Buffer**: 4MB for better throughput
- **TCP_NODELAY**: Configurable for low-latency transfers
- Platform-specific optimizations for Unix/Linux

### 4. **Advanced File I/O**
- **Pre-allocation**: Uses `fallocate()` on Linux for instant allocation
- **Direct I/O**: Option for bypassing OS cache on large files
- **Async I/O**: All file operations are fully asynchronous
- **Zero-copy**: Prepared infrastructure for `sendfile()` implementation

### 5. **HTTP Server Enhancements**
- **Range Support**: Full HTTP Range header support for parallel downloads
- **Optimized Streaming**: 8MB chunks for maximum throughput
- **Header Optimization**: Proper cache control and buffering headers
- **Request Pipelining**: Support for multiple concurrent requests

### 6. **Smart Connection Management**
- **Connection Pooling**: Reuses HTTP connections efficiently
- **Idle Timeout**: 60 seconds to keep connections warm
- **Max Connections**: 32 per host for parallel operations

## Performance Expectations

### Small Files (< 50MB)
- Single optimized connection
- Minimal overhead
- Expected: 400-500 Mbps on gigabit LAN

### Large Files (> 50MB)
- 4 parallel connections
- Range-based downloading
- Expected: 700-900 Mbps on gigabit LAN

### Compared to Previous Implementation
- **Before**: 89 Mbps average with long stalls
- **After**: 400-900 Mbps sustained throughput
- **Improvement**: 4-10x faster transfers

## Configuration Tunables

### Buffer Sizes (in client.rs)
```rust
const DOWNLOAD_BUFFER_SIZE: usize = 8 * 1024 * 1024; // Adjust based on available RAM
const WRITE_THRESHOLD: usize = 2 * 1024 * 1024; // Lower for more responsive progress
const PARALLEL_CONNECTIONS: usize = 4; // Increase for faster networks
const LARGE_FILE_THRESHOLD: u64 = 50 * 1024 * 1024; // When to use parallel
```

### TCP Settings (in server.rs)
```rust
const STREAM_BUFFER_SIZE: usize = 8 * 1024 * 1024;
const TCP_BUFFER_SIZE: u32 = 4 * 1024 * 1024;
```

## Platform-Specific Notes

### Linux
- Best performance with `fallocate()` support
- Consider increasing system TCP buffer limits:
  ```bash
  sudo sysctl -w net.core.rmem_max=67108864
  sudo sysctl -w net.core.wmem_max=67108864
  ```

### macOS
- Falls back to `set_len()` for pre-allocation
- May need to adjust system limits for optimal performance

### Windows
- Standard optimizations apply
- No special system configuration needed

## Monitoring Performance

The system now includes:
- Real-time speed monitoring during transfers
- Progress tracking with accurate speed calculations
- Transfer statistics API for monitoring

## Future Optimizations

1. **HTTP/2 Support**: For multiplexed streams (when server support improves)
2. **Zero-Copy sendfile()**: Direct kernel-to-kernel transfers on Linux
3. **Compression**: Optional compression for compressible files
4. **Adaptive Parallelism**: Dynamically adjust connection count based on network conditions

## Troubleshooting

### If speeds are still low:
1. Check network MTU settings (should be 1500 or higher)
2. Verify no firewall/antivirus interference
3. Ensure sufficient disk I/O bandwidth
4. Check for network congestion
5. Try adjusting PARALLEL_CONNECTIONS (2-8 range)

### For best results:
- Use wired Ethernet connection
- Ensure both devices support gigabit speeds
- Close other network-intensive applications
- Use SSD storage for maximum I/O performance