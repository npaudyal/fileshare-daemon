use tauri::command;
use crate::http::{OptimizedTransferManager, TransferBenchmark, test_optimized_transfers};
use tracing::info;

#[command]
pub async fn run_transfer_benchmark() -> Result<String, String> {
    info!("Starting transfer benchmark...");
    
    // Create temporary directory for benchmark
    let temp_dir = std::env::temp_dir().join("fileshare_benchmark");
    tokio::fs::create_dir_all(&temp_dir).await
        .map_err(|e| format!("Failed to create temp dir: {}", e))?;
    
    // Initialize optimized transfer manager
    let manager = OptimizedTransferManager::new(9879).await
        .map_err(|e| format!("Failed to create transfer manager: {}", e))?;
    
    // Start server
    manager.start_optimized_server().await
        .map_err(|e| format!("Failed to start server: {}", e))?;
    
    // Wait for server to be ready
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
    
    // Run benchmark
    let mut benchmark = TransferBenchmark::new();
    benchmark.run_benchmark(&manager, temp_dir.clone()).await
        .map_err(|e| format!("Benchmark failed: {}", e))?;
    
    // Cleanup
    tokio::fs::remove_dir_all(temp_dir).await.ok();
    
    // Get performance report
    let report = manager.get_performance_report().await;
    
    Ok(report)
}

#[command]
pub async fn get_optimization_info() -> Result<String, String> {
    Ok(format!(
        "HTTP Transfer Optimizations:\n\
         \n\
         📊 File Size Categories:\n\
         • Tiny (<1MB): 1 connection, HTTP/2, 64KB buffers\n\
         • Small (1-10MB): 2 connections, HTTP/2, 256KB buffers\n\
         • Medium (10-100MB): 4 connections, HTTP/1.1, 2MB buffers\n\
         • Large (100MB-1GB): 8 connections, HTTP/1.1, 8MB buffers\n\
         • Huge (>1GB): 16 connections, HTTP/1.1, 16MB buffers\n\
         \n\
         ⚡ Dynamic Features:\n\
         • Adaptive connection scaling based on network speed\n\
         • Zero-copy streaming for large files\n\
         • TCP socket optimization per file size\n\
         • Real-time performance monitoring\n\
         • Automatic rate adjustment\n\
         \n\
         🎯 Expected Performance:\n\
         • Small files: 400-600 Mbps\n\
         • Medium files: 600-800 Mbps\n\
         • Large files: 800-950 Mbps on gigabit LAN\n\
         • 10Gbps networks: Up to 8-9 Gbps"
    ))
}

#[command]
pub async fn test_transfer_optimizations() -> Result<String, String> {
    info!("🚀 Testing optimized transfer system...");
    
    test_optimized_transfers().await
        .map_err(|e| format!("Test failed: {}", e))
}