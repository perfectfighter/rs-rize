import { invoke, isTauri } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { installNativeBridge } from './native-bridge-core.js';

// Browser previews intentionally receive no native or storage capabilities.
if (typeof window !== 'undefined') installNativeBridge({ target: window, isNative: isTauri, invoke, listen });
