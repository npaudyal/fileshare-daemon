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

interface AdvancedSettingsProps {
    settings: any;
    onSettingsChange: (newSettings: any) => void;
}

const AdvancedSettings: React.FC<AdvancedSettingsProps> = ({ settings, onSettingsChange }) => {
    const [localSettings, setLocalSettings] = useState(settings);
    const [hasChanges, setHasChanges] = useState(false);
    const [showDeviceId, setShowDeviceId] = useState(false);
    const [isLoading, setIsLoading] = useState(false);
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
            <div className="bg-blue-500/10 border border-blue-500/20 rounded-lg p-4">
                <h3 className="text-blue-300 font-medium mb-3 flex items-center">
                    <SettingsIcon className="w-4 h-4 mr-2" />
                    Device Information
                </h3>
                <div className="space-y-3">
                    <div>
                        <label className="text-sm text-gray-300 block mb-1">Device Name</label>
                        <input
                            type="text"
                            value={localSettings.device_name || ''}
                            onChange={(e) => handleSettingChange('device_name', e.target.value)}
                            className="w-full px-3 py-2 bg-black/20 text-white rounded border border-white/20 focus:outline-none focus:border-blue-400"
                            placeholder="Enter device name"
                        />
                    </div>

                    <div>
                        <label className="text-sm text-gray-300 block mb-1">Device ID</label>
                        <div className="flex items-center space-x-2">
                            <div className="flex-1 px-3 py-2 bg-black/20 text-white rounded border border-white/10 font-mono text-xs">
                                {showDeviceId
                                    ? localSettings.device_id
                                    : localSettings.device_id?.substring(0, 8) + '...'
                                }
                            </div>
                            <button
                                onClick={() => setShowDeviceId(!showDeviceId)}
                                className="p-2 text-gray-400 hover:text-white transition-colors"
                            >
                                {showDeviceId ? <EyeOff className="w-4 h-4" /> : <Eye className="w-4 h-4" />}
                            </button>
                        </div>
                    </div>
                </div>
            </div>

            {/* Network Settings */}
            <div className="bg-green-500/10 border border-green-500/20 rounded-lg p-4">
                <h3 className="text-green-300 font-medium mb-3 flex items-center">
                    <Network className="w-4 h-4 mr-2" />
                    Network Configuration
                </h3>
                <div className="grid grid-cols-2 gap-3">
                    <div>
                        <label className="text-sm text-gray-300 block mb-1">Network Port</label>
                        <input
                            type="number"
                            value={localSettings.network_port || 9876}
                            onChange={(e) => handleSettingChange('network_port', parseInt(e.target.value))}
                            className={`w-full px-3 py-2 bg-black/20 text-white rounded border focus:outline-none ${validatePortRange(localSettings.network_port)
                                ? 'border-white/20 focus:border-green-400'
                                : 'border-red-500 focus:border-red-400'
                                }`}
                            min="1024"
                            max="65535"
                        />
                        {!validatePortRange(localSettings.network_port) && (
                            <p className="text-red-400 text-xs mt-1">Port must be between 1024-65535</p>
                        )}
                    </div>

                    <div>
                        <label className="text-sm text-gray-300 block mb-1">Discovery Port</label>
                        <input
                            type="number"
                            value={localSettings.discovery_port || 9877}
                            onChange={(e) => handleSettingChange('discovery_port', parseInt(e.target.value))}
                            className={`w-full px-3 py-2 bg-black/20 text-white rounded border focus:outline-none ${validatePortRange(localSettings.discovery_port)
                                ? 'border-white/20 focus:border-green-400'
                                : 'border-red-500 focus:border-red-400'
                                }`}
                            min="1024"
                            max="65535"
                        />
                    </div>
                </div>
            </div>

            {/* Transfer Settings */}
            <div className="bg-purple-500/10 border border-purple-500/20 rounded-lg p-4">
                <h3 className="text-purple-300 font-medium mb-3 flex items-center">
                    <Zap className="w-4 h-4 mr-2" />
                    Transfer Settings
                </h3>
                <div className="space-y-3">
                    <div>
                        <label className="text-sm text-gray-300 block mb-1">
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
                        <div className="flex justify-between text-xs text-gray-400">
                            <span>16KB</span>
                            <span>1MB</span>
                        </div>
                    </div>

                    <div>
                        <label className="text-sm text-gray-300 block mb-1">
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
                        <div className="flex justify-between text-xs text-gray-400">
                            <span>1</span>
                            <span>10</span>
                        </div>
                    </div>
                </div>
            </div>

            {/* Security Settings */}
            <div className="bg-red-500/10 border border-red-500/20 rounded-lg p-4">
                <h3 className="text-red-300 font-medium mb-3 flex items-center">
                    <Shield className="w-4 h-4 mr-2" />
                    Security Settings
                </h3>
                <div className="space-y-3">
                    <div className="flex items-center justify-between">
                        <div>
                            <span className="text-sm text-gray-300">Require Device Pairing</span>
                            <p className="text-xs text-gray-500">Only allow connections from paired devices</p>
                        </div>
                        <label className="relative inline-flex items-center cursor-pointer">
                            <input
                                type="checkbox"
                                checked={localSettings.require_pairing || false}
                                onChange={(e) => handleSettingChange('require_pairing', e.target.checked)}
                                className="sr-only peer"
                            />
                            <div className="w-11 h-6 bg-gray-600 peer-focus:outline-none rounded-full peer peer-checked:after:translate-x-full peer-checked:after:border-white after:content-[''] after:absolute after:top-[2px] after:left-[2px] after:bg-white after:rounded-full after:h-5 after:w-5 after:transition-all peer-checked:bg-blue-600"></div>
                        </label>
                    </div>

                    <div className="flex items-center justify-between">
                        <div>
                            <span className="text-sm text-gray-300">Enable Encryption</span>
                            <p className="text-xs text-gray-500">Encrypt all file transfers</p>
                        </div>
                        <label className="relative inline-flex items-center cursor-pointer">
                            <input
                                type="checkbox"
                                checked={localSettings.encryption_enabled || false}
                                onChange={(e) => handleSettingChange('encryption_enabled', e.target.checked)}
                                className="sr-only peer"
                            />
                            <div className="w-11 h-6 bg-gray-600 peer-focus:outline-none rounded-full peer peer-checked:after:translate-x-full peer-checked:after:border-white after:content-[''] after:absolute after:top-[2px] after:left-[2px] after:bg-white after:rounded-full after:h-5 after:w-5 after:transition-all peer-checked:bg-green-600"></div>
                        </label>
                    </div>

                    <div className="flex items-center justify-between">
                        <div>
                            <span className="text-sm text-gray-300">Auto-accept from Trusted</span>
                            <p className="text-xs text-gray-500">Automatically accept transfers from trusted devices</p>
                        </div>
                        <label className="relative inline-flex items-center cursor-pointer">
                            <input
                                type="checkbox"
                                checked={localSettings.auto_accept_from_trusted || false}
                                onChange={(e) => handleSettingChange('auto_accept_from_trusted', e.target.checked)}
                                className="sr-only peer"
                            />
                            <div className="w-11 h-6 bg-gray-600 peer-focus:outline-none rounded-full peer peer-checked:after:translate-x-full peer-checked:after:border-white after:content-[''] after:absolute after:top-[2px] after:left-[2px] after:bg-white after:rounded-full after:h-5 after:w-5 after:transition-all peer-checked:bg-yellow-600"></div>
                        </label>
                    </div>
                </div>
            </div>

            {/* Action Buttons */}
            <div className="flex items-center justify-between pt-4 border-t border-white/10">
                <div className="flex space-x-2">
                    <button
                        onClick={handleExportSettings}
                        className="flex items-center space-x-2 px-3 py-2 bg-blue-500/20 text-blue-300 rounded hover:bg-blue-500/30 transition-colors text-sm"
                    >
                        <Download className="w-4 h-4" />
                        <span>Export</span>
                    </button>

                    <button
                        onClick={handleImportSettings}
                        className="flex items-center space-x-2 px-3 py-2 bg-green-500/20 text-green-300 rounded hover:bg-green-500/30 transition-colors text-sm"
                    >
                        <Upload className="w-4 h-4" />
                        <span>Import</span>
                    </button>
                </div>

                <div className="flex space-x-2">
                    {hasChanges && (
                        <button
                            onClick={handleReset}
                            className="flex items-center space-x-2 px-4 py-2 bg-gray-500/20 text-gray-300 rounded hover:bg-gray-500/30 transition-colors"
                        >
                            <RotateCcw className="w-4 h-4" />
                            <span>Reset</span>
                        </button>
                    )}

                    <button
                        onClick={handleSave}
                        disabled={!hasChanges || isLoading}
                        className="flex items-center space-x-2 px-4 py-2 bg-green-500/20 text-green-300 rounded hover:bg-green-500/30 transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
                    >
                        {isLoading ? (
                            <div className="w-4 h-4 border-2 border-green-300 border-t-transparent rounded-full animate-spin" />
                        ) : (
                            <Save className="w-4 h-4" />
                        )}
                        <span>Save</span>
                    </button>
                </div>
            </div>

            {hasChanges && (
                <div className="flex items-center space-x-2 text-yellow-400 text-sm">
                    <AlertTriangle className="w-4 h-4" />
                    <span>You have unsaved changes</span>
                </div>
            )}
        </div>
    );
};

export default AdvancedSettings;