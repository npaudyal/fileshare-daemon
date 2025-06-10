import { useState, useEffect } from 'react';

export const useDebounce = <T>(value: T, delay: number): T => {
    const [debouncedValue, setDebouncedValue] = useState<T>(value);

    useEffect(() => {
        const handler = setTimeout(() => {
            setDebouncedValue(value);
        }, delay);

        return () => {
            clearTimeout(handler);
        };
    }, [value, delay]);

    return debouncedValue;
};

export const useThrottle = <T>(value: T, delay: number): T => {
    const [throttledValue, setThrottledValue] = useState<T>(value);
    const [lastUpdated, setLastUpdated] = useState<number>(Date.now());

    useEffect(() => {
        const now = Date.now();
        if (now - lastUpdated >= delay) {
            setThrottledValue(value);
            setLastUpdated(now);
        } else {
            const timeoutId = setTimeout(() => {
                setThrottledValue(value);
                setLastUpdated(Date.now());
            }, delay - (now - lastUpdated));

            return () => clearTimeout(timeoutId);
        }
    }, [value, delay, lastUpdated]);

    return throttledValue;
};