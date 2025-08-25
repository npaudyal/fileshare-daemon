use crate::network::protocol::Message;
use crate::{FileshareError, Result};
use quinn::{RecvStream, SendStream};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::{debug, error};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum StreamType {
    Control,        // For control messages (handshake, ping/pong, etc.)
    FileTransfer,   // For file chunk transfers
    Clipboard,      // For clipboard updates
    Progress,       // For progress updates
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuicMessage {
    pub stream_type: StreamType,
    pub message: Message,
}

impl QuicMessage {
    pub fn new(stream_type: StreamType, message: Message) -> Self {
        Self {
            stream_type,
            message,
        }
    }
    
    pub fn control(message: Message) -> Self {
        Self::new(StreamType::Control, message)
    }
    
    pub fn file_transfer(message: Message) -> Self {
        Self::new(StreamType::FileTransfer, message)
    }
}

pub struct QuicProtocol;

impl QuicProtocol {
    pub async fn write_message(stream: &mut SendStream, message: &Message) -> Result<()> {
        // Serialize the message
        let data = bincode::serialize(message)?;
        let len = data.len() as u32;
        
        debug!("Writing message of {} bytes, type: {:?}", len, message.message_type);
        
        // Write length prefix
        stream.write_all(&len.to_be_bytes()).await.map_err(|e| {
            FileshareError::Network(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                format!("Failed to write message length: {}", e)
            ))
        })?;
        
        // Write message data
        stream.write_all(&data).await.map_err(|e| {
            FileshareError::Network(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                format!("Failed to write message data: {}", e)
            ))
        })?;
        
        Ok(())
    }
    
    pub async fn read_message(stream: &mut RecvStream) -> Result<Message> {
        // Read length prefix
        let mut len_bytes = [0u8; 4];
        stream.read_exact(&mut len_bytes).await.map_err(|e| {
            FileshareError::Network(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                format!("Failed to read message length: {}", e)
            ))
        })?;
        
        let len = u32::from_be_bytes(len_bytes) as usize;
        
        // Validate message length  
        if len > 50_000_000 { // 50MB max
            error!("Message too large: {} bytes", len);
            return Err(FileshareError::Transfer(format!("Message too large: {} bytes", len)));
        }
        
        debug!("Reading message of {} bytes", len);
        
        // Read message data
        let mut data = vec![0u8; len];
        stream.read_exact(&mut data).await.map_err(|e| {
            FileshareError::Network(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                format!("Failed to read message data: {}", e)
            ))
        })?;
        
        // Deserialize message
        let message: Message = bincode::deserialize(&data)?;
        
        Ok(message)
    }
    
    pub async fn write_stream_header(stream: &mut SendStream, stream_type: StreamType) -> Result<()> {
        let header = bincode::serialize(&stream_type)?;
        let len = header.len() as u32;
        
        debug!("Writing stream header: {:?} ({} bytes)", stream_type, len);
        
        // Write length prefix
        stream.write_all(&len.to_be_bytes()).await.map_err(|e| {
            FileshareError::Network(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                format!("Failed to write stream header length: {}", e)
            ))
        })?;
        
        // Write header data
        stream.write_all(&header).await.map_err(|e| {
            FileshareError::Network(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                format!("Failed to write stream header data: {}", e)
            ))
        })?;
        Ok(())
    }
    
    pub async fn read_stream_header(stream: &mut RecvStream) -> Result<StreamType> {
        // Read the length of the serialized StreamType first
        let mut len_bytes = [0u8; 4];
        stream.read_exact(&mut len_bytes).await.map_err(|e| {
            FileshareError::Network(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                format!("Failed to read stream header length: {}", e)
            ))
        })?;
        
        let len = u32::from_be_bytes(len_bytes) as usize;
        
        // Validate header length (should be small)
        if len > 100 {
            error!("Stream header too large: {} bytes", len);
            return Err(FileshareError::Transfer(format!("Stream header too large: {} bytes", len)));
        }
        
        // Read the actual header data
        let mut header = vec![0u8; len];
        stream.read_exact(&mut header).await.map_err(|e| {
            FileshareError::Network(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                format!("Failed to read stream header data: {}", e)
            ))
        })?;
        
        let stream_type: StreamType = bincode::deserialize(&header)?;
        debug!("Read stream header: {:?} ({} bytes)", stream_type, len);
        Ok(stream_type)
    }
    
    // Optimized method for writing raw chunks without full message serialization
    pub async fn write_chunk_direct(
        stream: &mut SendStream,
        transfer_id: Uuid,
        chunk_index: u64,
        data: &[u8],
        is_last: bool,
    ) -> Result<()> {
        // Write header: transfer_id (16 bytes) + chunk_index (8 bytes) + data_len (4 bytes) + is_last (1 byte)
        let mut header = Vec::with_capacity(29);
        header.extend_from_slice(transfer_id.as_bytes());
        header.extend_from_slice(&chunk_index.to_be_bytes());
        header.extend_from_slice(&(data.len() as u32).to_be_bytes());
        header.push(if is_last { 1 } else { 0 });
        
        stream.write_all(&header).await.map_err(|e| {
            FileshareError::Network(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                format!("Failed to write chunk header: {}", e)
            ))
        })?;
        
        // Write chunk data
        stream.write_all(data).await.map_err(|e| {
            FileshareError::Network(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                format!("Failed to write chunk data: {}", e)
            ))
        })?;
        
        Ok(())
    }
    
    pub async fn read_chunk_direct(stream: &mut RecvStream) -> Result<(Uuid, u64, Vec<u8>, bool)> {
        // Read header
        let mut header = [0u8; 29];
        stream.read_exact(&mut header).await.map_err(|e| {
            FileshareError::Network(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                format!("Failed to read chunk header: {}", e)
            ))
        })?;
        
        // Parse header
        let transfer_id = Uuid::from_bytes(header[0..16].try_into().unwrap());
        let chunk_index = u64::from_be_bytes(header[16..24].try_into().unwrap());
        let data_len = u32::from_be_bytes(header[24..28].try_into().unwrap()) as usize;
        let is_last = header[28] != 0;
        
        // Validate chunk size
        if data_len > 32_000_000 { // 32MB max per chunk
            return Err(FileshareError::Transfer("Chunk too large".to_string()));
        }
        
        // Read chunk data
        let mut data = vec![0u8; data_len];
        stream.read_exact(&mut data).await.map_err(|e| {
            FileshareError::Network(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                format!("Failed to read chunk data: {}", e)
            ))
        })?;
        
        Ok((transfer_id, chunk_index, data, is_last))
    }
}