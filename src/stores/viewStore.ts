import { create } from 'zustand';
import { invoke } from '@tauri-apps/api/core';

export type WindowMode = 'Normal' | 'Transfer';

export interface Transfer {
    id: string;
    filename: string;
    file_path: string;
    source_device_id: string;
    source_device_name: string;
    target_path: string;
    file_size: number;
    transferred_bytes: number;
    speed_bps: number;
    eta_seconds?: number;
    status: 'Pending' | 'Connecting' | 'InProgress' | 'Paused' | 'Completed' | 'Failed' | 'Cancelled';
    direction: 'Incoming' | 'Outgoing';
    started_at: string;
    completed_at?: string;
    error?: string;
    is_paused: boolean;
}

interface ViewState {
    windowMode: WindowMode;
    transfers: Map<string, Transfer>;
    isTransitioning: boolean;

    // Actions
    setWindowMode: (mode: WindowMode, fromBackend?: boolean) => void;
    addTransfer: (transfer: Transfer) => void;
    updateTransfer: (id: string, updates: Partial<Transfer>) => void;
    removeTransfer: (id: string) => void;
    clearCompletedTransfers: () => void;
    setTransitioning: (transitioning: boolean) => void;

    // Helpers
    getActiveTransfers: () => Transfer[];
    getCompletedTransfers: () => Transfer[];
    hasActiveTransfers: () => boolean;
}

export const useViewStore = create<ViewState>((set, get) => ({
    windowMode: 'Normal',
    transfers: new Map(),
    isTransitioning: false,

    setWindowMode: async (mode: WindowMode, fromBackend: boolean = false) => {
        set({ windowMode: mode });
        // Only invoke backend if this change didn't originate from backend
        if (!fromBackend) {
            try {
                await invoke('set_window_mode', { mode });
            } catch (error) {
                console.error('Failed to set window mode:', error);
            }
        }
    },

    addTransfer: (transfer: Transfer) => {
        set((state) => {
            const newTransfers = new Map(state.transfers);
            newTransfers.set(transfer.id, transfer);
            return { transfers: newTransfers };
        });
    },

    updateTransfer: (id: string, updates: Partial<Transfer>) => {
        set((state) => {
            const newTransfers = new Map(state.transfers);
            const existing = newTransfers.get(id);
            if (existing) {
                newTransfers.set(id, { ...existing, ...updates });
            }
            return { transfers: newTransfers };
        });
    },

    removeTransfer: (id: string) => {
        set((state) => {
            const newTransfers = new Map(state.transfers);
            newTransfers.delete(id);
            return { transfers: newTransfers };
        });
    },

    clearCompletedTransfers: () => {
        set((state) => {
            const newTransfers = new Map();
            state.transfers.forEach((transfer, id) => {
                if (transfer.status !== 'Completed' && transfer.status !== 'Failed' && transfer.status !== 'Cancelled') {
                    newTransfers.set(id, transfer);
                }
            });
            return { transfers: newTransfers };
        });
    },

    setTransitioning: (transitioning: boolean) => {
        set({ isTransitioning: transitioning });
    },

    getActiveTransfers: () => {
        const transfers = Array.from(get().transfers.values());
        return transfers.filter(t =>
            t.status === 'Pending' ||
            t.status === 'Connecting' ||
            t.status === 'InProgress' ||
            t.status === 'Paused'
        );
    },

    getCompletedTransfers: () => {
        const transfers = Array.from(get().transfers.values());
        return transfers.filter(t =>
            t.status === 'Completed' ||
            t.status === 'Failed' ||
            t.status === 'Cancelled'
        );
    },

    hasActiveTransfers: () => {
        return get().getActiveTransfers().length > 0;
    }
}));