// Transfer-related types for the UI
export interface TransferInfo {
    id: string;
    fileName: string;
    fileSize: number;
    bytesTransferred: number;
    progress: number; // 0-100
    speed: number; // bytes per second
    eta: number | null; // seconds remaining
    state: TransferState;
    direction: 'Upload' | 'Download';
    peerName: string;
    error?: string;
    priority?: number; // 0=Low, 1=Normal, 2=High, 3=Critical
}

export interface TransferDetails extends TransferInfo {
    peerId: string;
    startedAt: number; // timestamp
    completedAt?: number; // timestamp
    retryCount: number;
    targetPath: string;
    checksum?: string;
}

export type TransferState =
    | 'Pending'
    | 'Connecting'
    | 'Negotiating'
    | 'Active'
    | 'Paused'
    | 'Stalled'
    | 'Resuming'
    | 'Finalizing'
    | 'Completed'
    | 'Failed'
    | 'Cancelled';

export interface TransferMetrics {
    totalTransfers: number;
    activeTransfers: number;
    completedTransfers: number;
    failedTransfers: number;
    totalBytesTransferred: number;
    currentBandwidthUsage: number;
    peakBandwidthUsage: number;
    averageSpeed: number;
    sessionStarted: number; // timestamp
}