import React, { useState, useEffect } from 'react';
import {
    Save,
    RotateCcw,
    Download,
    Upload,
    Shield,
    Zap,
    Network,
    Eye,
    EyeOff,
    AlertTriangle,
    Settings as SettingsIcon
} from 'lucide-react';
import { useToast } from '../hooks/useToast';
import { invoke } from '@tauri-apps/api/core';
import { useTheme } from '../context/ThemeContext';

interface AdvancedSettingsProps {
    settings: any;
    onSettingsChange: (newSettings: any) => void;
}

const AdvancedSettings: React.FC<AdvancedSettingsProps> = ({ settings, onSettingsChange }) => {
    const { theme } = useTheme();
    const [localSettings, setLocalSettings] = useState(settings);
    const [hasChanges, setHasChanges] = useState(false);
    const [showDeviceId, setShowDeviceId] = useState(false);
    const [isLoading, setIsLoading] = useState(false);
    const [hoveredButton, setHoveredButton] = useState<string | null>(null);
    const { addToast } = useToast();

    useEffect(() => {
        setLocalSettings(settings);
        setHasChanges(false);
    }, [settings]);

    const handleSettingChange = (key: string, value: any) => {
        const newSettings = { ...localSettings, [key]: value };
        setLocalSettings(newSettings);
        setHasChanges(true);
    };

    const handleSave = async () => {
        setIsLoading(true);
        try {
            await invoke('update_app_settings', { settings: localSettings });
            onSettingsChange(localSettings);
            setHasChanges(false);
            addToast('success', 'Settings Saved', 'Your settings have been saved successfully');
        } catch (error) {
            addToast('error', 'Save Failed', `Failed to save settings: ${error}`);
        } finally {
            setIsLoading(false);
        }
    };

    const handleReset = () => {
        setLocalSettings(settings);
        setHasChanges(false);
        addToast('info', 'Settings Reset', 'Settings have been reset to last saved values');
    };

    const handleExportSettings = async () => {
        try {
            await invoke('export_settings');
            addToast('success', 'Settings Exported', 'Settings exported to file successfully');
        } catch (error) {
            addToast('error', 'Export Failed', `Failed to export settings: ${error}`);
        }
    };

    const handleImportSettings = async () => {
        try {
            const imported = await invoke('import_settings');
            setLocalSettings(imported);
            setHasChanges(true);
            addToast('success', 'Settings Imported', 'Settings imported successfully');
        } catch (error) {
            addToast('error', 'Import Failed', `Failed to import settings: ${error}`);
        }
    };

    const validatePortRange = (port: number) => {
        return port >= 1024 && port <= 65535;
    };

    return (
        <div className="space-y-6">
            {/* Device Information */}
            <div
                className="rounded-lg p-4 border"
                style={{
                    backgroundColor: theme.colors.accent2 + '10',
                    borderColor: theme.colors.accent2 + '30'
                }}
            >
                <h3 className="font-medium mb-3 flex items-center" style={{ color: theme.colors.accent2 }}>
                    <SettingsIcon className="w-4 h-4 mr-2" />
                    Device Information
                </h3>
                <div className="space-y-3">
                    <div>
                        <label className="text-sm block mb-1" style={{ color: theme.colors.textSecondary }}>Device Name</label>
                        <input
                            type="text"
                            value={localSettings.device_name || ''}
                            onChange={(e) => handleSettingChange('device_name', e.target.value)}
                            className="w-full px-3 py-2 rounded border focus:outline-none"
                            style={{
                                backgroundColor: theme.colors.backgroundTertiary,
                                color: theme.colors.text,
                                borderColor: theme.colors.border
                            }}
                            onFocus={(e) => {
                                e.target.style.borderColor = theme.colors.accent2;
                            }}
                            onBlur={(e) => {
                                e.target.style.borderColor = theme.colors.border;
                            }}
                            placeholder="Enter device name"
                        />
                    </div>

                    <div>
                        <label className="text-sm block mb-1" style={{ color: theme.colors.textSecondary }}>Device ID</label>
                        <div className="flex items-center space-x-2">
                            <div
                                className="flex-1 px-3 py-2 rounded border font-mono text-xs"
                                style={{
                                    backgroundColor: theme.colors.backgroundTertiary,
                                    color: theme.colors.text,
                                    borderColor: theme.colors.border
                                }}
                            >
                                {showDeviceId
                                    ? localSettings.device_id
                                    : localSettings.device_id?.substring(0, 8) + '...'
                                }
                            </div>
                            <button
                                onClick={() => setShowDeviceId(!showDeviceId)}
                                className="p-2 transition-colors"
                                style={{ color: theme.colors.textSecondary }}
                                onMouseEnter={(e) => {
                                    e.currentTarget.style.color = theme.colors.text;
                                }}
                                onMouseLeave={(e) => {
                                    e.currentTarget.style.color = theme.colors.textSecondary;
                                }}
                            >
                                {showDeviceId ? <EyeOff className="w-4 h-4" /> : <Eye className="w-4 h-4" />}
                            </button>
                        </div>
                    </div>
                </div>
            </div>

            {/* Network Settings */}
            <div
                className="rounded-lg p-4 border"
                style={{
                    backgroundColor: theme.colors.success + '10',
                    borderColor: theme.colors.success + '30'
                }}
            >
                <h3 className="font-medium mb-3 flex items-center" style={{ color: theme.colors.success }}>
                    <Network className="w-4 h-4 mr-2" />
                    Network Configuration
                </h3>
                <div className="grid grid-cols-2 gap-3">
                    <div>
                        <label className="text-sm block mb-1" style={{ color: theme.colors.textSecondary }}>Network Port</label>
                        <input
                            type="number"
                            value={localSettings.network_port || 9876}
                            onChange={(e) => handleSettingChange('network_port', parseInt(e.target.value))}
                            className="w-full px-3 py-2 rounded border focus:outline-none"
                            style={{
                                backgroundColor: theme.colors.backgroundTertiary,
                                color: theme.colors.text,
                                borderColor: validatePortRange(localSettings.network_port) ? theme.colors.border : theme.colors.error
                            }}
                            onFocus={(e) => {
                                e.target.style.borderColor = validatePortRange(localSettings.network_port) ? theme.colors.success : theme.colors.error;
                            }}
                            onBlur={(e) => {
                                e.target.style.borderColor = validatePortRange(localSettings.network_port) ? theme.colors.border : theme.colors.error;
                            }}
                            min="1024"
                            max="65535"
                        />
                        {!validatePortRange(localSettings.network_port) && (
                            <p className="text-xs mt-1" style={{ color: theme.colors.error }}>Port must be between 1024-65535</p>
                        )}
                    </div>

                    <div>
                        <label className="text-sm block mb-1" style={{ color: theme.colors.textSecondary }}>Discovery Port</label>
                        <input
                            type="number"
                            value={localSettings.discovery_port || 9877}
                            onChange={(e) => handleSettingChange('discovery_port', parseInt(e.target.value))}
                            className="w-full px-3 py-2 rounded border focus:outline-none"
                            style={{
                                backgroundColor: theme.colors.backgroundTertiary,
                                color: theme.colors.text,
                                borderColor: validatePortRange(localSettings.discovery_port) ? theme.colors.border : theme.colors.error
                            }}
                            onFocus={(e) => {
                                e.target.style.borderColor = validatePortRange(localSettings.discovery_port) ? theme.colors.success : theme.colors.error;
                            }}
                            onBlur={(e) => {
                                e.target.style.borderColor = validatePortRange(localSettings.discovery_port) ? theme.colors.border : theme.colors.error;
                            }}
                            min="1024"
                            max="65535"
                        />
                    </div>
                </div>
            </div>

            {/* Transfer Settings */}
            <div
                className="rounded-lg p-4 border"
                style={{
                    backgroundColor: theme.colors.hover + '10',
                    borderColor: theme.colors.hover + '30'
                }}
            >
                <h3 className="font-medium mb-3 flex items-center" style={{ color: theme.colors.hover }}>
                    <Zap className="w-4 h-4 mr-2" />
                    Transfer Settings
                </h3>
                <div className="space-y-3">
                    <div>
                        <label className="text-sm block mb-1" style={{ color: theme.colors.textSecondary }}>
                            Chunk Size: {Math.round(localSettings.chunk_size / 1024)}KB
                        </label>
                        <input
                            type="range"
                            min="16384"
                            max="1048576"
                            step="16384"
                            value={localSettings.chunk_size || 65536}
                            onChange={(e) => handleSettingChange('chunk_size', parseInt(e.target.value))}
                            className="w-full"
                        />
                        <div className="flex justify-between text-xs" style={{ color: theme.colors.textTertiary }}>
                            <span>16KB</span>
                            <span>1MB</span>
                        </div>
                    </div>

                    <div>
                        <label className="text-sm block mb-1" style={{ color: theme.colors.textSecondary }}>
                            Max Concurrent Transfers: {localSettings.max_concurrent_transfers}
                        </label>
                        <input
                            type="range"
                            min="1"
                            max="10"
                            value={localSettings.max_concurrent_transfers || 5}
                            onChange={(e) => handleSettingChange('max_concurrent_transfers', parseInt(e.target.value))}
                            className="w-full"
                        />
                        <div className="flex justify-between text-xs" style={{ color: theme.colors.textTertiary }}>
                            <span>1</span>
                            <span>10</span>
                        </div>
                    </div>
                </div>
            </div>

            {/* Security Settings */}
            <div
                className="rounded-lg p-4 border"
                style={{
                    backgroundColor: theme.colors.error + '10',
                    borderColor: theme.colors.error + '30'
                }}
            >
                <h3 className="font-medium mb-3 flex items-center" style={{ color: theme.colors.error }}>
                    <Shield className="w-4 h-4 mr-2" />
                    Security Settings
                </h3>
                <div className="space-y-3">
                    <div className="flex items-center justify-between">
                        <div>
                            <span className="text-sm" style={{ color: theme.colors.textSecondary }}>Require Device Pairing</span>
                            <p className="text-xs" style={{ color: theme.colors.textTertiary }}>Only allow connections from paired devices</p>
                        </div>
                        <label className="relative inline-flex items-center cursor-pointer">
                            <input
                                type="checkbox"
                                checked={localSettings.require_pairing || false}
                                onChange={(e) => handleSettingChange('require_pairing', e.target.checked)}
                                className="sr-only peer"
                            />
                            <div
                                className="w-11 h-6 peer-focus:outline-none rounded-full peer peer-checked:after:translate-x-full after:content-[''] after:absolute after:top-[2px] after:left-[2px] after:rounded-full after:h-5 after:w-5 after:transition-all"
                                style={{
                                    backgroundColor: localSettings.require_pairing ? theme.colors.accent2 : theme.colors.backgroundTertiary,
                                    '--tw-after-bg-color': theme.colors.text,
                                    '--tw-after-border-color': theme.colors.text
                                } as any}
                            ></div>
                        </label>
                    </div>

                    <div className="flex items-center justify-between">
                        <div>
                            <span className="text-sm" style={{ color: theme.colors.textSecondary }}>Enable Encryption</span>
                            <p className="text-xs" style={{ color: theme.colors.textTertiary }}>Encrypt all file transfers</p>
                        </div>
                        <label className="relative inline-flex items-center cursor-pointer">
                            <input
                                type="checkbox"
                                checked={localSettings.encryption_enabled || false}
                                onChange={(e) => handleSettingChange('encryption_enabled', e.target.checked)}
                                className="sr-only peer"
                            />
                            <div
                                className="w-11 h-6 peer-focus:outline-none rounded-full peer peer-checked:after:translate-x-full after:content-[''] after:absolute after:top-[2px] after:left-[2px] after:rounded-full after:h-5 after:w-5 after:transition-all"
                                style={{
                                    backgroundColor: localSettings.encryption_enabled ? theme.colors.success : theme.colors.backgroundTertiary,
                                    '--tw-after-bg-color': theme.colors.text,
                                    '--tw-after-border-color': theme.colors.text
                                } as any}
                            ></div>
                        </label>
                    </div>

                    <div className="flex items-center justify-between">
                        <div>
                            <span className="text-sm" style={{ color: theme.colors.textSecondary }}>Auto-accept from Trusted</span>
                            <p className="text-xs" style={{ color: theme.colors.textTertiary }}>Automatically accept transfers from trusted devices</p>
                        </div>
                        <label className="relative inline-flex items-center cursor-pointer">
                            <input
                                type="checkbox"
                                checked={localSettings.auto_accept_from_trusted || false}
                                onChange={(e) => handleSettingChange('auto_accept_from_trusted', e.target.checked)}
                                className="sr-only peer"
                            />
                            <div
                                className="w-11 h-6 peer-focus:outline-none rounded-full peer peer-checked:after:translate-x-full after:content-[''] after:absolute after:top-[2px] after:left-[2px] after:rounded-full after:h-5 after:w-5 after:transition-all"
                                style={{
                                    backgroundColor: localSettings.auto_accept_from_trusted ? theme.colors.warning : theme.colors.backgroundTertiary,
                                    '--tw-after-bg-color': theme.colors.text,
                                    '--tw-after-border-color': theme.colors.text
                                } as any}
                            ></div>
                        </label>
                    </div>
                </div>
            </div>

            {/* Action Buttons */}
            <div className="flex items-center justify-between pt-4 border-t" style={{ borderColor: theme.colors.border }}>
                <div className="flex space-x-2">
                    <button
                        onClick={handleExportSettings}
                        onMouseEnter={() => setHoveredButton('export')}
                        onMouseLeave={() => setHoveredButton(null)}
                        className="flex items-center space-x-2 px-3 py-2 rounded transition-colors text-sm"
                        style={{
                            backgroundColor: hoveredButton === 'export' ? theme.colors.accent2 + '30' : theme.colors.accent2 + '20',
                            color: theme.colors.accent2
                        }}
                    >
                        <Download className="w-4 h-4" />
                        <span>Export</span>
                    </button>

                    <button
                        onClick={handleImportSettings}
                        onMouseEnter={() => setHoveredButton('import')}
                        onMouseLeave={() => setHoveredButton(null)}
                        className="flex items-center space-x-2 px-3 py-2 rounded transition-colors text-sm"
                        style={{
                            backgroundColor: hoveredButton === 'import' ? theme.colors.success + '30' : theme.colors.success + '20',
                            color: theme.colors.success
                        }}
                    >
                        <Upload className="w-4 h-4" />
                        <span>Import</span>
                    </button>
                </div>

                <div className="flex space-x-2">
                    {hasChanges && (
                        <button
                            onClick={handleReset}
                            onMouseEnter={() => setHoveredButton('reset')}
                            onMouseLeave={() => setHoveredButton(null)}
                            className="flex items-center space-x-2 px-4 py-2 rounded transition-colors"
                            style={{
                                backgroundColor: hoveredButton === 'reset' ? theme.colors.textSecondary + '30' : theme.colors.textSecondary + '20',
                                color: theme.colors.textSecondary
                            }}
                        >
                            <RotateCcw className="w-4 h-4" />
                            <span>Reset</span>
                        </button>
                    )}

                    <button
                        onClick={handleSave}
                        disabled={!hasChanges || isLoading}
                        onMouseEnter={() => setHoveredButton('save')}
                        onMouseLeave={() => setHoveredButton(null)}
                        className="flex items-center space-x-2 px-4 py-2 rounded transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
                        style={{
                            backgroundColor: (!hasChanges || isLoading)
                                ? theme.colors.backgroundTertiary
                                : (hoveredButton === 'save' ? theme.colors.success + '30' : theme.colors.success + '20'),
                            color: (!hasChanges || isLoading) ? theme.colors.textTertiary : theme.colors.success
                        }}
                    >
                        {isLoading ? (
                            <div
                                className="w-4 h-4 border-2 border-t-transparent rounded-full animate-spin"
                                style={{ borderColor: theme.colors.success, borderTopColor: 'transparent' }}
                            />
                        ) : (
                            <Save className="w-4 h-4" />
                        )}
                        <span>Save</span>
                    </button>
                </div>
            </div>

            {hasChanges && (
                <div className="flex items-center space-x-2 text-sm" style={{ color: theme.colors.warning }}>
                    <AlertTriangle className="w-4 h-4" />
                    <span>You have unsaved changes</span>
                </div>
            )}
        </div>
    );
};

export default AdvancedSettings;