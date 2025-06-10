import React, { createContext, useContext, useState, useCallback } from 'react';
import ToastComponent, { Toast, ToastType } from '../components/Toast';

interface ToastContextType {
    addToast: (type: ToastType, title: string, message: string, options?: { duration?: number; icon?: React.ReactNode }) => void;
    removeToast: (id: string) => void;
}

const ToastContext = createContext<ToastContextType | undefined>(undefined);

export const ToastProvider: React.FC<{ children: React.ReactNode }> = ({ children }) => {
    const [toasts, setToasts] = useState<Toast[]>([]);

    const addToast = useCallback((
        type: ToastType,
        title: string,
        message: string,
        options?: { duration?: number; icon?: React.ReactNode }
    ) => {
        const id = Date.now().toString();
        const newToast: Toast = {
            id,
            type,
            title,
            message,
            duration: options?.duration,
            icon: options?.icon,
        };

        setToasts(prev => [...prev, newToast]);
    }, []);

    const removeToast = useCallback((id: string) => {
        setToasts(prev => prev.filter(toast => toast.id !== id));
    }, []);

    return (
        <ToastContext.Provider value={{ addToast, removeToast }}>
            {children}

            {/* Toast Container */}
            <div className="fixed bottom-4 right-4 z-50 space-y-2 max-w-sm">
                {toasts.map(toast => (
                    <ToastComponent
                        key={toast.id}
                        toast={toast}
                        onRemove={removeToast}
                    />
                ))}
            </div>
        </ToastContext.Provider>
    );
};

export const useToast = () => {
    const context = useContext(ToastContext);
    if (context === undefined) {
        throw new Error('useToast must be used within a ToastProvider');
    }
    return context;
};