import React from 'react';
import { motion, AnimatePresence } from 'framer-motion';
import { useViewStore } from '../stores/viewStore';

interface ViewTransitionProps {
    children: React.ReactNode;
    mode: 'Normal' | 'Transfer';
}

const ViewTransition: React.FC<ViewTransitionProps> = ({ children, mode }) => {
    const { windowMode, isTransitioning } = useViewStore();

    const variants = {
        initial: {
            opacity: 0,
            scale: 0.95,
            y: mode === 'Transfer' ? 20 : -20,
        },
        animate: {
            opacity: 1,
            scale: 1,
            y: 0,
        },
        exit: {
            opacity: 0,
            scale: 0.95,
            y: mode === 'Transfer' ? -20 : 20,
        }
    };

    return (
        <AnimatePresence mode="wait">
            {windowMode === mode && (
                <motion.div
                    key={mode}
                    initial="initial"
                    animate="animate"
                    exit="exit"
                    variants={variants}
                    transition={{
                        duration: 0.3,
                        ease: [0.4, 0, 0.2, 1]
                    }}
                    style={{
                        width: '100%',
                        height: '100%',
                        position: isTransitioning ? 'absolute' : 'relative',
                        top: 0,
                        left: 0
                    }}
                >
                    {children}
                </motion.div>
            )}
        </AnimatePresence>
    );
};

export default ViewTransition;