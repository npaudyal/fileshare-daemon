import React, { useState, useEffect } from 'react';
import { CheckCircle, AlertCircle, Info, X, Wifi, WifiOff } from 'lucide-react';

export type ToastType = 'success' | 'error' | 'info' | 'warning';

export interface Toast {
    id: string;
    type: ToastType;
    title: string;
    message: string;
    duration?: number;
    icon?: React.ReactNode;
}

interface ToastProps {
    toast: Toast;
    onRemove: (id: string) => void;
}

const ToastComponent: React.FC<ToastProps> = ({ toast, onRemove }) => {
    const [isVisible, setIsVisible] = useState(false);

    useEffect(() => {
        setIsVisible(true);
        const timer = setTimeout(() => {
            setIsVisible(false);
            setTimeout(() => onRemove(toast.id), 300);
        }, toast.duration || 4000);

        return () => clearTimeout(timer);
    }, [toast.id, toast.duration, onRemove]);

    const getToastStyles = () => {
        switch (toast.type) {
            case 'success':
                return 'bg-green-500/20 border-green-500/50 text-green-300';
            case 'error':
                return 'bg-red-500/20 border-red-500/50 text-red-300';
            case 'warning':
                return 'bg-yellow-500/20 border-yellow-500/50 text-yellow-300';
            default:
                return 'bg-blue-500/20 border-blue-500/50 text-blue-300';
        }
    };

    const getIcon = () => {
        if (toast.icon) return toast.icon;

        switch (toast.type) {
            case 'success':
                return <CheckCircle className="w-5 h-5" />;
            case 'error':
                return <AlertCircle className="w-5 h-5" />;
            case 'warning':
                return <AlertCircle className="w-5 h-5" />;
            default:
                return <Info className="w-5 h-5" />;
        }
    };

    return (
        <div
            className={`
        ${getToastStyles()}
        ${isVisible ? 'animate-slide-up opacity-100' : 'opacity-0'}
        backdrop-blur-sm rounded-lg border p-4 shadow-lg transition-all duration-300
        transform ${isVisible ? 'translate-y-0' : 'translate-y-2'}
      `}
        >
            <div className="flex items-start space-x-3">
                <div className="flex-shrink-0">
                    {getIcon()}
                </div>
                <div className="flex-1 min-w-0">
                    <p className="text-sm font-medium">{toast.title}</p>
                    <p className="text-xs text-gray-400 mt-1">{toast.message}</p>
                </div>
                <button
                    onClick={() => onRemove(toast.id)}
                    className="flex-shrink-0 text-gray-400 hover:text-white transition-colors"
                >
                    <X className="w-4 h-4" />
                </button>
            </div>
        </div>
    );
};

export default ToastComponent;