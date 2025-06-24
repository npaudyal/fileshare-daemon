# Windows Build Instructions

If you encounter errors with `aws-lc-sys` on Windows, here are several solutions:

## Solution 1: Clean Build
```bash
cargo clean
cargo build
```

## Solution 2: Environment Variables
Set these environment variables before building:
```bash
set RUSTFLAGS=-C target-feature=+crt-static
set AWS_LC_SYS_NO_PREFIX=1
cargo build
```

## Solution 3: Force Ring Backend
The Cargo.toml has been configured to use the `ring` crypto backend instead of `aws-lc-rs`. If you still see issues, you can force it with:
```bash
cargo build --features ring
```

## Solution 4: Install Build Tools (if needed)
If the above don't work, install:
1. Visual Studio Build Tools 2019 or later
2. CMake
3. NASM (for assembly optimizations)

## Solution 5: Use Native Dependencies
```bash
cargo build --no-default-features --features ring,runtime-tokio,tls-rustls
```

The current configuration should work without additional tools, but these are backup options.