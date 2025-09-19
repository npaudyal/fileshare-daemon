import { useEffect, useRef } from 'react';
import { listen } from '@tauri-apps/api/event';
import { invoke } from '@tauri-apps/api/core';
import { useViewStore, Transfer, WindowMode } from '../stores/viewStore';

interface TransferStartedEvent {
    transfer_id: string;
    filename: string;
    file_size: number;
    source_device_name: string;
    direction: 'Incoming' | 'Outgoing';
}

interface TransferProgressEvent {
    transfer_id: string;
    progress_percent: number;
    speed_mbps: number;
    eta_seconds?: number;
    transferred_mb: number;
    total_mb: number;
    status: Transfer['status'];
}

interface TransferCompletedEvent {
    transfer_id: string;
    filename: string;
    success: boolean;
    error?: string;
}

export const useTransferView = () => {
    const {
        windowMode,
        transfers,
        isTransitioning,
        setWindowMode,
        addTransfer,
        updateTransfer,
        removeTransfer,
        setTransitioning,
        getActiveTransfers,
        hasActiveTransfers
    } = useViewStore();

    const unlistenersRef = useRef<Array<() => void>>([]);
    const autoReturnTimerRef = useRef<number | null>(null);

    useEffect(() => {
        const setupListeners = async () => {
            // Listen for transfer started events
            const unlistenStart = await listen<TransferStartedEvent>('transfer-started', (event) => {
                console.log('Transfer started:', event.payload);

                const transfer: Transfer = {
                    id: event.payload.transfer_id,
                    filename: event.payload.filename,
                    file_path: '',
                    source_device_id: '',
                    source_device_name: event.payload.source_device_name,
                    target_path: '',
                    file_size: event.payload.file_size,
                    transferred_bytes: 0,
                    speed_bps: 0,
                    eta_seconds: undefined,
                    status: 'Connecting',
                    direction: event.payload.direction,
                    started_at: new Date().toISOString(),
                    completed_at: undefined,
                    error: undefined,
                    is_paused: false
                };

                addTransfer(transfer);

                // Auto-switch to transfer view
                if (windowMode === 'Normal') {
                    setTransitioning(true);
                    setTimeout(() => {
                        setWindowMode('Transfer');
                        setTransitioning(false);
                    }, 300);
                }
            });

            // Listen for transfer progress events
            const unlistenProgress = await listen<TransferProgressEvent>('transfer-progress', (event) => {
                const payload = event.payload;

                updateTransfer(payload.transfer_id, {
                    transferred_bytes: payload.transferred_mb * 1024 * 1024,
                    speed_bps: payload.speed_mbps * 1024 * 1024 * 8,
                    eta_seconds: payload.eta_seconds,
                    status: payload.status
                });
            });

            // Listen for transfer completed events
            const unlistenComplete = await listen<TransferCompletedEvent>('transfer-completed', (event) => {
                console.log('Transfer completed:', event.payload);

                updateTransfer(event.payload.transfer_id, {
                    status: event.payload.success ? 'Completed' : 'Failed',
                    completed_at: new Date().toISOString(),
                    error: event.payload.error
                });

                // Check if all transfers are complete and auto-return to normal view
                setTimeout(() => {
                    if (!hasActiveTransfers() && windowMode === 'Transfer') {
                        // Clear any existing timer
                        if (autoReturnTimerRef.current) {
                            clearTimeout(autoReturnTimerRef.current);
                        }

                        // Set timer to return to normal view after 3 seconds
                        autoReturnTimerRef.current = window.setTimeout(() => {
                            setTransitioning(true);
                            setTimeout(() => {
                                setWindowMode('Normal');
                                setTransitioning(false);
                            }, 300);
                        }, 3000);
                    }
                }, 100);
            });

            // Listen for window mode changes from backend
            const unlistenMode = await listen<WindowMode>('window-mode-changed', (event) => {
                console.log('Window mode changed:', event.payload);
                if (event.payload !== windowMode) {
                    setWindowMode(event.payload);
                }
            });

            // Store unlisteners
            unlistenersRef.current = [
                unlistenStart,
                unlistenProgress,
                unlistenComplete,
                unlistenMode
            ];

            // Load initial state
            try {
                const currentMode = await invoke<WindowMode>('get_window_mode');
                setWindowMode(currentMode);

                const activeTransfers = await invoke<Transfer[]>('get_active_transfers');
                activeTransfers.forEach(transfer => {
                    addTransfer(transfer);
                });
            } catch (error) {
                console.error('Failed to load initial state:', error);
            }
        };

        setupListeners();

        // Cleanup
        return () => {
            unlistenersRef.current.forEach(unlisten => unlisten());
            if (autoReturnTimerRef.current) {
                clearTimeout(autoReturnTimerRef.current);
            }
        };
    }, []);

    // Manual mode toggle
    const toggleWindowMode = async () => {
        const newMode = windowMode === 'Normal' ? 'Transfer' : 'Normal';
        setTransitioning(true);
        setTimeout(() => {
            setWindowMode(newMode);
            setTransitioning(false);
        }, 300);
    };

    // Cancel all transfers and return to normal view
    const cancelAllAndReturn = async () => {
        const activeTransfers = getActiveTransfers();
        for (const transfer of activeTransfers) {
            try {
                await invoke('cancel_transfer', { transferId: transfer.id });
            } catch (error) {
                console.error(`Failed to cancel transfer ${transfer.id}:`, error);
            }
        }

        setTransitioning(true);
        setTimeout(() => {
            setWindowMode('Normal');
            setTransitioning(false);
        }, 300);
    };

    return {
        windowMode,
        transfers: Array.from(transfers.values()),
        isTransitioning,
        toggleWindowMode,
        cancelAllAndReturn,
        hasActiveTransfers: hasActiveTransfers()
    };
};