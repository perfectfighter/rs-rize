// Keep the renderer boundary narrow: every method maps to one Rust command.
// The injected adapter lets tests exercise the real bridge without a webview.
export function normalizeNativeError(value) {
  if (value instanceof Error) return value;
  if (typeof value === 'string' && value.trim()) return new Error(value);
  if (value && typeof value.message === 'string' && value.message.trim()) {
    const error = new Error(value.message);
    if (typeof value.code === 'string') error.code = value.code;
    return error;
  }
  return new Error('The desktop operation failed. Please retry.');
}

export function createNativeBridge({ invoke, listen, onListenerError = error => console.error('Native event subscription failed:', error.message) }) {
  if (typeof invoke !== 'function' || typeof listen !== 'function') throw new TypeError('A native command and event adapter is required.');
  const call = async (command, args) => {
    try { return await invoke(command, args); }
    catch (error) { throw normalizeNativeError(error); }
  };
  const report = value => onListenerError(normalizeNativeError(value));
  const subscribe = (channel, callback) => {
    if (typeof callback !== 'function') throw new TypeError('An event callback is required.');
    let active = true;
    let unlisten;
    const cleanup = () => {
      if (!unlisten) return;
      const stop = unlisten;
      unlisten = undefined;
      try { Promise.resolve(stop()).catch(report); } catch (error) { report(error); }
    };
    try {
      Promise.resolve(listen(channel, event => { if (active) callback(event.payload); }))
        .then(stop => {
          if (typeof stop !== 'function') throw new Error('The native event listener could not be registered.');
          unlisten = stop;
          if (!active) cleanup();
        })
        .catch(report);
    } catch (error) { report(error); }
    return () => { active = false; cleanup(); };
  };
  return Object.freeze({
    getState: () => call('rize_get_state'),
    getStatus: () => call('rize_get_status'),
    mutate: command => call('rize_mutate', { command }),
    onState: callback => subscribe('rize:state', callback),
    onStatus: callback => subscribe('rize:status', callback),
    windowAction: action => call('rize_window', { action }),
    exportData: format => call('rize_export', { format }),
    importData: () => call('rize_import'),
    cloud: (action, payload = {}) => call('rize_cloud', { action, payload }),
    businessReport: (query = {}) => call('rize_business_report', { query }),
    invoicePdf: payload => call('rize_invoice_pdf', { payload }),
    updates: action => call('rize_updates', { action }),
  });
}

export function installNativeBridge({ target = globalThis, isNative, ...adapter }) {
  if (!isNative()) return null;
  const bridge = createNativeBridge(adapter);
  Object.defineProperty(target, 'rize', { value: bridge, enumerable: true, writable: false, configurable: false });
  return bridge;
}
