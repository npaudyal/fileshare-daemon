import React, { useState, useEffect } from 'react';
import { CheckCircle, AlertCircle, Info, X } from 'lucide-react';
import { useTheme } from '../context/ThemeContext';

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
    const { theme } = useTheme();
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
                return {
                    backgroundColor: theme.colors.success + '20',
                    borderColor: theme.colors.success + '80',
                    color: theme.colors.success
                };
            case 'error':
                return {
                    backgroundColor: theme.colors.error + '20',
                    borderColor: theme.colors.error + '80',
                    color: theme.colors.error
                };
            case 'warning':
                return {
                    backgroundColor: theme.colors.warning + '20',
                    borderColor: theme.colors.warning + '80',
                    color: theme.colors.warning
                };
            default:
                return {
                    backgroundColor: theme.colors.info + '20',
                    borderColor: theme.colors.info + '80',
                    color: theme.colors.info
                };
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

    const toastStyles = getToastStyles();

    return (
        <div
            className={`backdrop-blur-sm rounded-lg border p-4 shadow-lg transition-all duration-300 transform ${
                isVisible
                    ? 'animate-slide-up opacity-100 translate-y-0'
                    : 'opacity-0 translate-y-2'
            }`}
            style={{
                ...toastStyles,
                boxShadow: `0 10px 25px ${theme.colors.shadow}`
            }}
        >
            <div className="flex items-start space-x-3">
                <div className="flex-shrink-0">
                    {getIcon()}
                </div>
                <div className="flex-1 min-w-0">
                    <p className="text-sm font-medium">{toast.title}</p>
                    <p className="text-xs mt-1" style={{ color: theme.colors.textSecondary }}>{toast.message}</p>
                </div>
                <button
                    onClick={() => onRemove(toast.id)}
                    className="flex-shrink-0 transition-colors"
                    style={{ color: theme.colors.textSecondary }}
                    onMouseEnter={(e) => {
                        e.currentTarget.style.color = theme.colors.text;
                    }}
                    onMouseLeave={(e) => {
                        e.currentTarget.style.color = theme.colors.textSecondary;
                    }}
                >
                    <X className="w-4 h-4" />
                </button>
            </div>
        </div>
    );
};

export default ToastComponent;