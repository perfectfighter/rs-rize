//! Windows desktop shell. Privileged operations stay behind the original narrow bridge.
use crate::{pdf, updates::UpdateService};
use rize_cloud::{CloudCancellation, CloudClient};
use rize_core::{
    business,
    state::{self, Store},
    tracking::TrackingEngine,
};
use rize_native::{Sampler, SessionEvent, SessionWatcher};
use serde_json::{json, Value};
use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, MutexGuard,
    },
    time::{Duration, Instant},
};
use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Listener, Manager, State, WebviewUrl, WebviewWindow, WebviewWindowBuilder,
    WindowEvent,
};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
use tauri_plugin_opener::OpenerExt;

type Result<T> = std::result::Result<T, String>;
struct TrackingRuntime {
    engine: TrackingEngine,
    sampler: Sampler,
    watcher: Option<SessionWatcher>,
    locked: bool,
    suspended: bool,
    storage_failed: bool,
    observer_failed: bool,
    last_sample: Instant,
}
impl Default for TrackingRuntime {
    fn default() -> Self {
        Self {
            engine: TrackingEngine::default(),
            sampler: Sampler::default(),
            watcher: None,
            locked: false,
            suspended: false,
            storage_failed: false,
            observer_failed: false,
            last_sample: Instant::now() - Duration::from_secs(5),
        }
    }
}
struct Desktop {
    store: Arc<Mutex<Store>>,
    tracking: Mutex<TrackingRuntime>,
    cloud: tokio::sync::Mutex<CloudClient>,
    cancellation: CloudCancellation,
    updates: tokio::sync::Mutex<UpdateService>,
    status: Mutex<Value>,
    quitting: AtomicBool,
    stopped: AtomicBool,
    shutdown_started: AtomicBool,
    exit_ready: AtomicBool,
    tray: AtomicBool,
}
fn lock<'a, T>(value: &'a Mutex<T>, label: &str) -> Result<MutexGuard<'a, T>> {
    value.lock().map_err(|_| {
        format!("{label} is unavailable. Restart the application; existing files are preserved.")
    })
}
fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}
fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}
fn workspace(desktop: &Desktop) -> Result<Value> {
    Ok(lock(&desktop.store, "Workspace")?.get_state())
}
fn publish_state(app: &AppHandle, desktop: &Desktop) {
    if let Ok(value) = workspace(desktop) {
        let _ = app.emit_to("main", "rize:state", value);
    }
}
fn publish_status(app: &AppHandle, desktop: &Desktop) -> Value {
    // Sampling can publish while holding the collector lock. Never wait for the
    // GUI thread here: its Quit callback may be waiting to flush that collector.
    let value = desktop
        .status
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    let tooltip = format!(
        "Rize Local — {}",
        value["tracking"]["state"].as_str().unwrap_or("off")
    );
    let tooltip_app = app.clone();
    let _ = app.run_on_main_thread(move || {
        if let Some(tray) = tooltip_app.tray_by_id("main-tray") {
            let _ = tray.set_tooltip(Some(tooltip));
        }
    });
    let _ = app.emit_to("main", "rize:status", &value);
    value
}
fn tracking_status(app: &AppHandle, desktop: &Desktop, patch: Value) {
    let mut status = desktop.status.lock().unwrap_or_else(|e| e.into_inner());
    for (k, v) in patch.as_object().unwrap() {
        status["tracking"][k] = v.clone();
    }
    drop(status);
    publish_status(app, desktop);
}
fn cloud_status(app: &AppHandle, desktop: &Desktop, value: Value) {
    desktop.status.lock().unwrap_or_else(|e| e.into_inner())["cloud"] = value;
    publish_status(app, desktop);
}
async fn pause_cloud(app: &AppHandle, desktop: &Desktop) -> Result<()> {
    // This handle is outside the asynchronous client lock, so in-flight replies
    // are cancelled before this reset waits behind them.
    desktop.cancellation.cancel();
    let (result, status) = {
        let mut cloud = desktop.cloud.lock().await;
        let result = cloud.pause_sync();
        (result, cloud.status())
    };
    cloud_status(app, desktop, status);
    result
}
fn persist_samples(app: &AppHandle, desktop: &Desktop, entries: Vec<Value>) -> Result<()> {
    if !entries.is_empty() {
        lock(&desktop.store, "Workspace")?.upsert_activity(&entries, now_ms())?;
        publish_state(app, desktop);
    }
    Ok(())
}
fn flush_locked(app: &AppHandle, desktop: &Desktop, runtime: &mut TrackingRuntime) -> Result<()> {
    let current = workspace(desktop)?;
    persist_samples(app, desktop, runtime.engine.flush(&now_iso(), &current))
}
fn flush_activity(app: &AppHandle, desktop: &Desktop) -> Result<()> {
    let mut runtime = lock(&desktop.tracking, "Activity collection")?;
    flush_locked(app, desktop, &mut runtime)?;
    runtime.engine.reset();
    Ok(())
}
fn fail_tracking(app: &AppHandle, desktop: &Desktop) {
    if let Ok(mut runtime) = desktop.tracking.lock() {
        runtime.sampler.stop();
        runtime.engine.reset();
        runtime.storage_failed = true;
    }
    tracking_status(
        app,
        desktop,
        json!({"state":"error","message":"Activity could not be saved. Turn tracking off and on to retry."}),
    );
}
fn record_sample(
    app: &AppHandle,
    desktop: &Desktop,
    runtime: &mut TrackingRuntime,
    sample: Value,
) -> Result<()> {
    let current = workspace(desktop)?;
    let entries = runtime.engine.consume(&sample, &current);
    persist_samples(app, desktop, entries)?;
    tracking_status(
        app,
        desktop,
        json!({"available":sample["available"]==true,"state":if sample["available"]==true{"tracking"}else{"unavailable"},"message":if sample["available"]==true{"Foreground collection is enabled. Idle, schedule, and exclusion rules apply."}else{"Windows foreground information is temporarily unavailable; retrying."},"lastSampleAt":sample["timestamp"]}),
    );
    Ok(())
}
fn reconcile_tracking(app: &AppHandle, desktop: &Desktop) -> Result<()> {
    let current = workspace(desktop)?;
    let enabled = current["onboarded"] == true
        && current["demo"] != true
        && current["settings"]["trackingEnabled"] == true;
    desktop.status.lock().unwrap_or_else(|e| e.into_inner())["tracking"]["enabled"] =
        json!(enabled);
    let mut runtime = lock(&desktop.tracking, "Activity collection")?;
    if !enabled {
        runtime.sampler.stop();
        flush_locked(app, desktop, &mut runtime)?;
        runtime.engine.reset();
        runtime.storage_failed = false;
        runtime.observer_failed = false;
        runtime.locked = false;
        runtime.suspended = false;
        runtime.watcher = None;
        drop(runtime);
        tracking_status(
            app,
            desktop,
            json!({"state":"off","message":if current["demo"]==true{"Sample workspace does not collect activity."}else{"Automatic tracking is off."}}),
        );
        return Ok(());
    }
    if !cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        runtime.sampler.stop();
        flush_locked(app, desktop, &mut runtime)?;
        drop(runtime);
        tracking_status(
            app,
            desktop,
            json!({"available":false,"state":"unavailable","message":"Automatic foreground tracking is available in the Windows x64 app."}),
        );
        return Ok(());
    }
    if runtime.storage_failed {
        drop(runtime);
        publish_status(app, desktop);
        return Ok(());
    }
    if runtime.watcher.is_none() && !runtime.observer_failed {
        match SessionWatcher::start() {
            Ok(watcher) => runtime.watcher = Some(watcher),
            Err(_) => runtime.observer_failed = true,
        }
    }
    if runtime.observer_failed {
        runtime.sampler.stop();
        flush_locked(app, desktop, &mut runtime)?;
        runtime.engine.reset();
        drop(runtime);
        tracking_status(
            app,
            desktop,
            json!({"available":false,"state":"unavailable","message":"Windows lock notifications are unavailable. Turn tracking off and on to retry."}),
        );
        return Ok(());
    }
    if runtime.locked || runtime.suspended {
        runtime.sampler.stop();
        flush_locked(app, desktop, &mut runtime)?;
        runtime.engine.reset();
        let sleeping = runtime.suspended;
        drop(runtime);
        tracking_status(
            app,
            desktop,
            json!({"state":"paused","message":if sleeping{"Tracking pauses while the computer sleeps."}else{"Tracking pauses while Windows is locked."}}),
        );
        return Ok(());
    }
    if runtime.sampler.status()["running"] == true {
        drop(runtime);
        publish_status(app, desktop);
        return Ok(());
    }
    tracking_status(
        app,
        desktop,
        json!({"state":"starting","message":"Starting Windows foreground tracking."}),
    );
    runtime.sampler.set_suspended(false);
    let sample = runtime.sampler.start();
    runtime.last_sample = Instant::now();
    record_sample(app, desktop, &mut runtime, sample)
}
fn native_tick(app: &AppHandle, desktop: &Desktop) -> Result<()> {
    let mut runtime = lock(&desktop.tracking, "Activity collection")?;
    let mut events = Vec::new();
    if let Some(watcher) = &runtime.watcher {
        while let Some(event) = watcher.try_recv() {
            events.push(event)
        }
        if !watcher.is_running() {
            runtime.observer_failed = true;
            runtime.sampler.stop();
            flush_locked(app, desktop, &mut runtime)?;
            runtime.engine.reset();
        }
    }
    // Drain all boundaries. A lock followed by unlock between polls still breaks continuity.
    for event in &events {
        flush_locked(app, desktop, &mut runtime)?;
        runtime.engine.reset();
        runtime.sampler.stop();
        match event {
            SessionEvent::Lock => runtime.locked = true,
            SessionEvent::Unlock => runtime.locked = false,
            SessionEvent::Suspend => runtime.suspended = true,
            SessionEvent::Resume => runtime.suspended = false,
        }
    }
    if !events.is_empty() || runtime.observer_failed {
        drop(runtime);
        return reconcile_tracking(app, desktop);
    }
    if runtime.sampler.status()["running"] == true
        && !runtime.locked
        && !runtime.suspended
        && !runtime.storage_failed
        && runtime.last_sample.elapsed() >= Duration::from_secs(5)
    {
        let sample = runtime.sampler.sample();
        runtime.last_sample = Instant::now();
        record_sample(app, desktop, &mut runtime, sample)?;
    }
    Ok(())
}
fn allowed_origin(url: &url::Url, debug: bool) -> bool {
    if !url.username().is_empty() || url.password().is_some() {
        return false;
    }
    let packaged = matches!(
        (url.scheme(), url.host_str()),
        ("http", Some("tauri.localhost")) | ("tauri", Some("localhost"))
    ) && url.port().is_none();
    let dev = debug
        && url.scheme() == "http"
        && matches!(url.host_str(), Some("localhost" | "127.0.0.1"))
        && url.port_or_known_default() == Some(1420);
    packaged || dev
}
fn allowed_page(url: &url::Url, debug: bool) -> bool {
    allowed_origin(url, debug) && matches!(url.path(), "/" | "/index.html")
}
fn assert_caller(window: &WebviewWindow, desktop: &Desktop) -> Result<()> {
    if desktop.quitting.load(Ordering::SeqCst) {
        return Err("The application is closing. Existing work is being saved.".into());
    }
    if window.label() != "main"
        || !window
            .url()
            .is_ok_and(|url| allowed_page(&url, cfg!(debug_assertions)))
    {
        return Err("This request is not from the app window.".into());
    }
    Ok(())
}
fn validate_json_size(value: &Value, limit: usize, message: &str) -> Result<()> {
    if serde_json::to_vec(value)
        .map_err(|_| message.to_string())?
        .len()
        > limit
    {
        return Err(message.into());
    }
    Ok(())
}
fn authorization_url(value: &Value) -> Result<url::Url> {
    let target = value
        .as_str()
        .ok_or("The server returned an unsupported authorization address.")?;
    let url = url::Url::parse(target)
        .map_err(|_| "The server returned an unsupported authorization address.")?;
    let google = url.host_str() == Some("accounts.google.com") && url.path() == "/o/oauth2/v2/auth";
    let segments = url.path().split('/').collect::<Vec<_>>();
    let microsoft = url.host_str() == Some("login.microsoftonline.com")
        && segments.len() == 5
        && segments[0].is_empty()
        && !segments[1].is_empty()
        && segments[1]
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'.' || c == b'-')
        && segments[2..] == ["oauth2", "v2.0", "authorize"];
    if url.scheme() != "https"
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
        || (!google && !microsoft)
    {
        return Err("The server returned an unsupported authorization address.".into());
    }
    Ok(url)
}

#[tauri::command]
async fn rize_get_state(window: WebviewWindow, desktop: State<'_, Arc<Desktop>>) -> Result<Value> {
    assert_caller(&window, &desktop)?;
    workspace(&desktop)
}
#[tauri::command]
async fn rize_get_status(
    app: AppHandle,
    window: WebviewWindow,
    desktop: State<'_, Arc<Desktop>>,
) -> Result<Value> {
    assert_caller(&window, &desktop)?;
    let maximized = window.is_maximized().unwrap_or(false);
    desktop.status.lock().unwrap_or_else(|e| e.into_inner())["maximized"] = json!(maximized);
    Ok(publish_status(&app, &desktop))
}
#[tauri::command]
async fn rize_mutate(
    app: AppHandle,
    window: WebviewWindow,
    desktop: State<'_, Arc<Desktop>>,
    command: Value,
) -> Result<Value> {
    assert_caller(&window, &desktop)?;
    if !command.is_object() || !command["type"].is_string() {
        return Err("Invalid workspace command.".into());
    }
    validate_json_size(&command, 200000, "Invalid workspace command.")?;
    if matches!(
        command["type"].as_str(),
        Some("workspace.create" | "workspace.demo")
    ) {
        pause_cloud(&app, &desktop).await?;
    }
    flush_activity(&app, &desktop)?;
    let result = lock(&desktop.store, "Workspace")?.mutate(&command, now_ms())?;
    publish_state(&app, &desktop);
    if reconcile_tracking(&app, &desktop).is_err() {
        fail_tracking(&app, &desktop)
    }
    Ok(result)
}
#[tauri::command]
async fn rize_window(
    app: AppHandle,
    window: WebviewWindow,
    desktop: State<'_, Arc<Desktop>>,
    action: String,
) -> Result<bool> {
    assert_caller(&window, &desktop)?;
    match action.as_str() {
        "minimize" => window.minimize(),
        "maximize" => {
            if window.is_maximized().map_err(|e| e.to_string())? {
                window.unmaximize()
            } else {
                window.maximize()
            }
        }
        "close" => window.close(),
        _ => return Err("Invalid window action.".into()),
    }
    .map_err(|_| "The window action could not be completed.")?;
    publish_status(&app, &desktop);
    Ok(true)
}
#[tauri::command]
async fn rize_cloud(
    app: AppHandle,
    window: WebviewWindow,
    desktop: State<'_, Arc<Desktop>>,
    action: String,
    payload: Value,
) -> Result<Value> {
    assert_caller(&window, &desktop)?;
    validate_json_size(
        &json!({"action":action,"payload":payload}),
        4 * 1024 * 1024,
        "Invalid cloud command.",
    )?;
    let ticket = desktop.cancellation.ticket()?;
    if action == "sync" {
        flush_activity(&app, &desktop)?
    }
    let (result, status) = {
        let mut cloud = desktop.cloud.lock().await;
        let result = cloud.execute_with_ticket(&action, payload, ticket).await;
        (result, cloud.status())
    };
    cloud_status(&app, &desktop, status);
    let result = result?;
    if action == "integrations.oauth.start" {
        let target = authorization_url(&result["result"]["authorizationUrl"])?;
        app.opener()
            .open_url(target.as_str(), None::<&str>)
            .map_err(|_| "The authorization page could not be opened in your browser.")?;
    }
    if result.get("workspace").is_some() {
        publish_state(&app, &desktop)
    }
    Ok(result)
}
#[tauri::command]
async fn rize_business_report(
    window: WebviewWindow,
    desktop: State<'_, Arc<Desktop>>,
    query: Value,
) -> Result<Value> {
    assert_caller(&window, &desktop)?;
    validate_json_size(&query, 200000, "Invalid report query.")?;
    let query = if query.is_null() { json!({}) } else { query };
    business::financial_summary(&workspace(&desktop)?, &query)
}
fn write_selected_file(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .filter(|p| p.is_dir())
        .ok_or("The selected folder is unavailable.")?;
    let temp = parent.join(format!(".rs-rize-export-{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| -> std::io::Result<()> {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temp, path)
    })();
    if let Err(error) = result {
        let _ = fs::remove_file(&temp);
        return Err(format!("The export could not be saved: {error}"));
    }
    Ok(())
}
#[tauri::command]
async fn rize_export(
    app: AppHandle,
    window: WebviewWindow,
    desktop: State<'_, Arc<Desktop>>,
    format: String,
) -> Result<Value> {
    assert_caller(&window, &desktop)?;
    if !["json", "csv"].contains(&format.as_str()) {
        return Err("Choose JSON or CSV export.".into());
    }
    let name = format!(
        "rize-local-{}.{}",
        chrono::Utc::now().format("%Y-%m-%d"),
        format
    );
    let selected = app
        .dialog()
        .file()
        .set_parent(&window)
        .set_title(if format == "json" {
            "Save workspace backup"
        } else {
            "Export all workspace entries"
        })
        .set_file_name(name)
        .add_filter(format.to_uppercase(), &[format.as_str()])
        .blocking_save_file();
    let Some(selected) = selected else {
        return Ok(json!({"canceled":true}));
    };
    let path = selected
        .into_path()
        .map_err(|_| "Choose a local file destination.")?;
    assert_caller(&window, &desktop)?;
    flush_activity(&app, &desktop)?;
    let snapshot = lock(&desktop.store, "Workspace")?.export_snapshot(now_ms())?;
    let text = if format == "json" {
        format!(
            "{}\n",
            serde_json::to_string_pretty(&snapshot)
                .map_err(|_| "The workspace backup could not be prepared.")?
        )
    } else {
        state::to_csv_checked(&snapshot)?
    };
    write_selected_file(&path, text.as_bytes())?;
    Ok(json!({"canceled":false,"path":path}))
}
#[tauri::command]
async fn rize_import(
    app: AppHandle,
    window: WebviewWindow,
    desktop: State<'_, Arc<Desktop>>,
) -> Result<Value> {
    assert_caller(&window, &desktop)?;
    let selected = app
        .dialog()
        .file()
        .set_parent(&window)
        .set_title("Open Rize Local workspace backup")
        .add_filter("Workspace JSON", &["json"])
        .blocking_pick_file();
    let Some(selected) = selected else {
        return Ok(json!({"canceled":true}));
    };
    let path = selected
        .into_path()
        .map_err(|_| "Choose a local workspace JSON file.")?;
    let file = fs::File::open(path).map_err(|_| "The selected backup could not be opened.")?;
    let mut bytes = Vec::new();
    file.take(20 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "The selected backup could not be read.")?;
    if bytes.len() > 20 * 1024 * 1024 {
        return Err("This workspace backup exceeds the 20 MB import limit.".into());
    }
    let mut value: Value =
        serde_json::from_slice(&bytes).map_err(|_| "The selected file is not valid JSON.")?;
    let accepted=app.dialog().message("Importing replaces the current local workspace.\n\nExport a JSON backup first if you want to keep the current data. Automatic tracking in the imported workspace starts disabled.").title("Replace this workspace?").kind(MessageDialogKind::Warning).buttons(MessageDialogButtons::OkCancelCustom("Replace workspace".into(),"Cancel".into())).parent(&window).blocking_show();
    if !accepted {
        return Ok(json!({"canceled":true}));
    }
    assert_caller(&window, &desktop)?;
    pause_cloud(&app, &desktop).await?;
    if value.is_object() {
        value["workspaceId"] = json!(uuid::Uuid::new_v4().to_string());
        if value["settings"].is_object() {
            value["settings"]["trackingEnabled"] = json!(false)
        }
    }
    // Validate before changing the collector or replacing any live data.
    let clean = state::validate_snapshot(&value)?;
    if clean["timer"]["status"] == "running" {
        return Err("Imported timers must be paused to avoid counting time since export.".into());
    }
    flush_activity(&app, &desktop)?;
    {
        let mut runtime = lock(&desktop.tracking, "Activity collection")?;
        runtime.sampler.stop();
        runtime.engine.reset();
    }
    let imported = lock(&desktop.store, "Workspace")?.import_data(clean, now_ms());
    let imported = match imported {
        Ok(value) => value,
        Err(error) => {
            if reconcile_tracking(&app, &desktop).is_err() {
                fail_tracking(&app, &desktop)
            }
            return Err(error);
        }
    };
    if let Ok(mut runtime) = desktop.tracking.lock() {
        runtime.storage_failed = false;
    }
    publish_state(&app, &desktop);
    if reconcile_tracking(&app, &desktop).is_err() {
        fail_tracking(&app, &desktop)
    }
    Ok(json!({"canceled":false,"state":imported}))
}
#[tauri::command]
async fn rize_invoice_pdf(
    app: AppHandle,
    window: WebviewWindow,
    desktop: State<'_, Arc<Desktop>>,
    payload: Value,
) -> Result<Value> {
    assert_caller(&window, &desktop)?;
    validate_json_size(&payload, 200000, "Invalid invoice selection.")?;
    let snapshot = workspace(&desktop)?;
    let invoice = snapshot["business"]["invoices"]
        .as_array()
        .and_then(|a| a.iter().find(|i| i["id"] == payload["id"]))
        .ok_or("This invoice no longer exists.")?;
    let html = business::invoice_html(invoice)?;
    let name = invoice["number"]
        .as_str()
        .unwrap_or("invoice")
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '-') {
                c
            } else {
                '-'
            }
        })
        .collect::<String>();
    let selected = app
        .dialog()
        .file()
        .set_parent(&window)
        .set_title("Export invoice PDF")
        .set_file_name(format!("{name}.pdf"))
        .add_filter("PDF", &["pdf"])
        .blocking_save_file();
    let Some(selected) = selected else {
        return Ok(json!({"canceled":true}));
    };
    let path = selected
        .into_path()
        .map_err(|_| "Choose a local PDF destination.")?;
    assert_caller(&window, &desktop)?;
    pdf::export_pdf(&app, html, path.clone()).await?;
    Ok(json!({"canceled":false,"path":path}))
}
#[tauri::command]
async fn rize_updates(
    app: AppHandle,
    window: WebviewWindow,
    desktop: State<'_, Arc<Desktop>>,
    action: String,
) -> Result<Value> {
    assert_caller(&window, &desktop)?;
    if !["status", "check", "download", "install"].contains(&action.as_str()) {
        return Err("Invalid update action.".into());
    }
    if action == "install" {
        if desktop.updates.lock().await.status()["state"] != "downloaded" {
            return Err("Download and verify an update before installing.".into());
        }
        desktop.cancellation.cancel();
        {
            let mut cloud = desktop.cloud.lock().await;
            let result = cloud.pause_sync();
            let mut status = cloud.status();
            if let Err(error) = result {
                status["message"] = json!(error)
            }
            cloud_status(&app, &desktop, status);
        }
        flush_activity(&app, &desktop)?;
        lock(&desktop.store, "Workspace")?.flush(now_ms())?;
        desktop.quitting.store(true, Ordering::SeqCst);
    }
    let result = {
        let mut updates = desktop.updates.lock().await;
        match action.as_str() {
            "status" => Ok(updates.status()),
            "check" => updates.check().await,
            "download" => updates.download().await,
            "install" => updates.install().await,
            _ => unreachable!(),
        }
    };
    if action == "install" && result.is_err() {
        desktop.quitting.store(false, Ordering::SeqCst)
    }
    if let Ok(value) = &result {
        desktop.status.lock().unwrap_or_else(|e| e.into_inner())["updates"] = value.clone();
        publish_status(&app, &desktop);
    }
    result
}

async fn secure_main_webview(window: &WebviewWindow) -> Result<()> {
    use webview2_com::{
        Microsoft::Web::WebView2::Win32::{
            COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL, COREWEBVIEW2_WEB_RESOURCE_CONTEXT_MEDIA,
        },
        WebResourceRequestedEventHandler,
    };
    use windows::{
        core::{w, PWSTR},
        Win32::System::Com::{CoTaskMemFree, IStream},
    };
    let (sender, receiver) = tokio::sync::oneshot::channel();
    window.with_webview(move|platform|unsafe{
        let operation=(||->windows::core::Result<()>{
            let core=platform.controller().CoreWebView2()?;let environment=platform.environment();let settings=core.Settings()?;
            settings.SetAreHostObjectsAllowed(false)?;settings.SetAreDefaultContextMenusEnabled(false)?;settings.SetAreDevToolsEnabled(cfg!(debug_assertions))?;
            // Script dialogs remain available because existing deletion flows use window.confirm.
            core.AddWebResourceRequestedFilter(w!("*"),COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL)?;
            let blocker=WebResourceRequestedEventHandler::create(Box::new(move|_,args|{
                if let Some(args)=args{
                    let request=args.Request()?;let mut raw=PWSTR::null();request.Uri(&mut raw)?;
                    let address=if raw.is_null(){String::new()}else{let value=raw.to_string().unwrap_or_default();CoTaskMemFree(Some(raw.as_ptr().cast()));value};
                    let local=url::Url::parse(&address).is_ok_and(|u|allowed_origin(&u,cfg!(debug_assertions))||((matches!((u.scheme(),u.host_str()),("http",Some("ipc.localhost"))|("ipc",Some("localhost"))))&&u.port().is_none()&&u.username().is_empty()&&u.password().is_none()));
                    let mut context=COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL;args.ResourceContext(&mut context)?;
                    let image=address.starts_with("data:image/");let media=address.starts_with("blob:")&&context==COREWEBVIEW2_WEB_RESOURCE_CONTEXT_MEDIA;
                    if !local&&!image&&!media{let response=environment.CreateWebResourceResponse(None::<&IStream>,403,w!("Blocked"),w!("Content-Type: text/plain"))?;args.SetResponse(&response)?;}
                }Ok(())
            }));let mut token=0;core.add_WebResourceRequested(&blocker,&mut token)?;Ok(())
        })();let _=sender.send(operation.map_err(|_|"The app window could not enable its local network policy. Existing workspace files are preserved.".to_string()));
    }).map_err(|_|"The app window could not initialize its security policy.")?;
    tokio::time::timeout(Duration::from_secs(30), receiver)
        .await
        .map_err(|_| "The app window took too long to initialize.")?
        .map_err(|_| "The app window closed during initialization.")?
}
async fn create_main_window(app: &AppHandle, desktop: &Arc<Desktop>) -> Result<()> {
    if let Some(window) = app.get_webview_window("main") {
        window
            .show()
            .map_err(|_| "The application window could not be opened.")?;
        let _ = window.unminimize();
        let _ = window.set_focus();
        return Ok(());
    }
    let address = if cfg!(debug_assertions) {
        WebviewUrl::External("http://localhost:1420".parse().unwrap())
    } else {
        WebviewUrl::App("index.html".into())
    };
    let window = WebviewWindowBuilder::new(app, "main", address)
        .title("Rize Local")
        .inner_size(1440., 940.)
        .min_inner_size(940., 650.)
        .decorations(false)
        .visible(false)
        .use_https_scheme(false)
        .devtools(cfg!(debug_assertions))
        .background_color(tauri::webview::Color(18, 17, 23, 255))
        .disable_drag_drop_handler()
        .on_navigation(|url| allowed_page(url, cfg!(debug_assertions)))
        .on_new_window(|_, _| tauri::webview::NewWindowResponse::Deny)
        .on_permission_request(|_, _| tauri::webview::PermissionResponse::Deny)
        .on_download(|_, _| false)
        .build()
        .map_err(|error| format!("The application window could not open: {error}"))?;
    if let Err(error) = secure_main_webview(&window).await {
        let _ = window.destroy();
        return Err(error);
    }
    window
        .show()
        .map_err(|_| "The application window could not be shown.")?;
    let _ = window.set_focus();
    publish_status(app, desktop);
    Ok(())
}
fn show_main(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}
fn create_tray(app: &AppHandle, desktop: &Desktop) -> Result<()> {
    let icon = app
        .default_window_icon()
        .ok_or("The app icon is unavailable.")?
        .clone();
    let open = MenuItem::with_id(app, "open", "Open Rize Local", true, None::<&str>)
        .map_err(|e| e.to_string())?;
    let separator = PredefinedMenuItem::separator(app).map_err(|e| e.to_string())?;
    let quit = MenuItem::with_id(app, "quit", "Quit Rize Local", true, None::<&str>)
        .map_err(|e| e.to_string())?;
    let menu = Menu::with_items(app, &[&open, &separator, &quit]).map_err(|e| e.to_string())?;
    TrayIconBuilder::with_id("main-tray")
        .icon(icon)
        .menu(&menu)
        .show_menu_on_left_click(false)
        .tooltip("Rize Local — off")
        .on_menu_event(|app, event| match event.id().as_ref() {
            "open" => show_main(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if matches!(
                event,
                TrayIconEvent::DoubleClick {
                    button: MouseButton::Left,
                    ..
                }
            ) {
                show_main(tray.app_handle())
            }
        })
        .build(app)
        .map_err(|e| e.to_string())?;
    desktop.tray.store(true, Ordering::SeqCst);
    Ok(())
}
fn start_background(app: AppHandle, desktop: Arc<Desktop>) -> Result<()> {
    let native_app = app.clone();
    let native_desktop = desktop.clone();
    std::thread::Builder::new()
        .name("rs-rize-activity".into())
        .spawn(move || {
            let mut checkpoint = Instant::now();
            while !native_desktop.stopped.load(Ordering::SeqCst) {
                if native_tick(&native_app, &native_desktop).is_err() {
                    fail_tracking(&native_app, &native_desktop)
                }
                if checkpoint.elapsed() >= Duration::from_secs(15) {
                    checkpoint = Instant::now();
                    let running =
                        workspace(&native_desktop).is_ok_and(|s| s["timer"]["status"] == "running");
                    if running
                        && lock(&native_desktop.store, "Workspace")
                            .and_then(|mut s| s.flush(now_ms()))
                            .is_err()
                    {
                        fail_tracking(&native_app, &native_desktop)
                    }
                }
                std::thread::sleep(Duration::from_millis(250));
            }
        })
        .map_err(|_| "The activity scheduler could not be started.")?;
    tauri::async_runtime::spawn(async move {
        while !desktop.stopped.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_secs(60)).await;
            if desktop.stopped.load(Ordering::SeqCst) {
                break;
            }
            let current = match workspace(&desktop) {
                Ok(s) => s,
                Err(_) => continue,
            };
            if current["onboarded"] != true || current["demo"] == true {
                continue;
            }
            // Consent is explicit in CloudClient and must remain valid after this operation queues.
            let ticket = match desktop.cancellation.ticket() {
                Ok(t) => t,
                Err(_) => continue,
            };
            let mut cloud = desktop.cloud.lock().await;
            if cloud.status()["autoSync"] != true {
                continue;
            }
            if flush_activity(&app, &desktop).is_err() {
                drop(cloud);
                fail_tracking(&app, &desktop);
                continue;
            }
            let result = cloud
                .execute_with_ticket("sync", json!({"consent":true}), ticket)
                .await;
            let mut status = cloud.status();
            if let Err(error) = &result {
                status["message"] = json!(error)
            }
            drop(cloud);
            cloud_status(&app, &desktop, status);
            if result.is_ok() {
                publish_state(&app, &desktop)
            }
        }
    });
    Ok(())
}
fn shutdown(app: &AppHandle, desktop: &Arc<Desktop>) {
    if desktop.shutdown_started.swap(true, Ordering::SeqCst) {
        return;
    }
    desktop.quitting.store(true, Ordering::SeqCst);
    desktop.stopped.store(true, Ordering::SeqCst);
    desktop.cancellation.cancel();
    // Workspace durability never depends on credential storage or a stalled cloud request.
    if let Err(error) = flush_activity(app, desktop) {
        eprintln!("Activity could not be saved during shutdown: {error}")
    }
    if let Ok(mut runtime) = desktop.tracking.lock() {
        runtime.sampler.stop();
        runtime.engine.reset();
        runtime.watcher = None;
    }
    if let Err(error) = lock(&desktop.store, "Workspace").and_then(|mut s| s.flush(now_ms())) {
        eprintln!("Workspace could not be saved during shutdown: {error}")
    }
    let app = app.clone();
    let desktop = desktop.clone();
    tauri::async_runtime::spawn(async move {
        let result = desktop.cloud.lock().await.pause_sync();
        if let Err(error) = result {
            eprintln!("Cloud synchronization could not be paused: {error}")
        }
        desktop.exit_ready.store(true, Ordering::SeqCst);
        app.exit(0);
    });
}
fn initialize(app: &AppHandle) -> Result<Arc<Desktop>> {
    let directory = app
        .path()
        .app_data_dir()
        .map_err(|_| "The local application data folder is unavailable.")?;
    fs::create_dir_all(&directory)
        .map_err(|_| "The local application data folder could not be created.")?;
    let store = Arc::new(Mutex::new(Store::new(Some(
        directory.join("workspace.json"),
    ))?));
    let cloud = CloudClient::new(Some(directory.join("cloud-session.json")), store.clone())?;
    let cancellation = cloud.cancel_handle();
    let cloud_status = cloud.status();
    let updates = UpdateService::new(app.clone());
    let updates_status = updates.status();
    Ok(Arc::new(Desktop {
        store,
        tracking: Mutex::new(TrackingRuntime::default()),
        cloud: tokio::sync::Mutex::new(cloud),
        cancellation,
        updates: tokio::sync::Mutex::new(updates),
        status: Mutex::new(
            json!({"platform":"win32","appVersion":app.package_info().version.to_string(),"tracking":{"enabled":false,"available":cfg!(target_arch="x86_64"),"state":"off","message":"Automatic tracking is off.","lastSampleAt":null},"maximized":false,"cloud":cloud_status,"updates":updates_status}),
        ),
        quitting: AtomicBool::new(false),
        stopped: AtomicBool::new(false),
        shutdown_started: AtomicBool::new(false),
        exit_ready: AtomicBool::new(false),
        tray: AtomicBool::new(false),
    }))
}
fn startup_error(app: &AppHandle, message: String) {
    let app = app.clone();
    std::thread::spawn(move || {
        app.dialog()
            .message(format!(
                "{message}\n\nExisting workspace files have been preserved."
            ))
            .title("Rize Local could not start")
            .kind(MessageDialogKind::Error)
            .blocking_show();
        app.exit(1);
    });
}
pub fn run() {
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _, _| {
            show_main(app)
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .invoke_handler(tauri::generate_handler![
            rize_get_state,
            rize_get_status,
            rize_mutate,
            rize_window,
            rize_export,
            rize_import,
            rize_cloud,
            rize_business_report,
            rize_invoice_pdf,
            rize_updates
        ])
        .setup(|app| {
            let handle = app.handle().clone();
            let desktop = match initialize(&handle) {
                Ok(d) => d,
                Err(error) => {
                    startup_error(&handle, error);
                    return Ok(());
                }
            };
            app.manage(desktop.clone());
            let status_app = handle.clone();
            let status_desktop = desktop.clone();
            app.listen("rize:update-status", move |event| {
                if let Ok(value) = serde_json::from_str::<Value>(event.payload()) {
                    status_desktop
                        .status
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())["updates"] = value;
                    publish_status(&status_app, &status_desktop);
                }
            });
            let _ = create_tray(&handle, &desktop);
            tauri::async_runtime::spawn(async move {
                match create_main_window(&handle, &desktop).await {
                    Ok(()) => {
                        if reconcile_tracking(&handle, &desktop).is_err() {
                            fail_tracking(&handle, &desktop)
                        }
                        if let Err(error) = start_background(handle.clone(), desktop) {
                            startup_error(&handle, error)
                        }
                    }
                    Err(error) => startup_error(&handle, error),
                }
            });
            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() != "main" {
                return;
            }
            let Some(desktop) = window.app_handle().try_state::<Arc<Desktop>>() else {
                return;
            };
            match event {
                WindowEvent::CloseRequested { api, .. }
                    if desktop.tray.load(Ordering::SeqCst)
                        && !desktop.quitting.load(Ordering::SeqCst) =>
                {
                    api.prevent_close();
                    let _ = window.hide();
                }
                WindowEvent::Resized(_) => {
                    let maximized = window.is_maximized().unwrap_or(false);
                    desktop.status.lock().unwrap_or_else(|e| e.into_inner())["maximized"] =
                        json!(maximized);
                    publish_status(window.app_handle(), &desktop);
                }
                _ => {}
            }
        });
    match builder.build(tauri::generate_context!()) {
        Ok(app) => app.run(|app, event| match event {
            tauri::RunEvent::ExitRequested { api, .. } => {
                if let Some(desktop) = app.try_state::<Arc<Desktop>>() {
                    if !desktop.exit_ready.load(Ordering::SeqCst) {
                        api.prevent_exit();
                        if !desktop.shutdown_started.load(Ordering::SeqCst) {
                            shutdown(app, &desktop)
                        }
                    }
                }
            }
            tauri::RunEvent::Exit => {
                if let Some(desktop) = app.try_state::<Arc<Desktop>>() {
                    desktop.stopped.store(true, Ordering::SeqCst);
                    desktop.cancellation.cancel();
                    let _ = flush_activity(app, &desktop);
                    if let Ok(mut store) = desktop.store.lock() {
                        let _ = store.flush(now_ms());
                    }
                }
            }
            _ => {}
        }),
        Err(error) => eprintln!(
            "Rize Local could not start: {error}. Existing workspace files have been preserved."
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ipc_page_origin_is_exact_and_excludes_external_and_print_windows() {
        for address in [
            "http://tauri.localhost/index.html",
            "tauri://localhost/index.html",
        ] {
            assert!(allowed_page(&address.parse().unwrap(), false))
        }
        for address in [
            "https://example.com/index.html",
            "http://tauri.localhost.evil/index.html",
            "https://tauri.localhost/index.html",
            "http://tauri.localhost:9000/index.html",
            "http://user:secret@tauri.localhost/index.html",
            "data:text/html,hello",
            "http://localhost:1420/index.html",
        ] {
            assert!(
                !allowed_page(&address.parse().unwrap(), false),
                "accepted {address}"
            )
        }
        assert!(allowed_page(
            &"http://localhost:1420/index.html".parse().unwrap(),
            true
        ));
        assert!(!allowed_page(
            &"http://localhost:1421/index.html".parse().unwrap(),
            true
        ));
    }
    #[test]
    fn oauth_opener_only_accepts_existing_google_and_microsoft_authorization_paths() {
        for address in [
            "https://accounts.google.com/o/oauth2/v2/auth?client_id=example",
            "https://login.microsoftonline.com/common/oauth2/v2.0/authorize?client_id=example",
        ] {
            assert!(authorization_url(&json!(address)).is_ok())
        }
        for address in [
            "https://example.com/authorize",
            "http://accounts.google.com/o/oauth2/v2/auth",
            "https://accounts.google.com/redirect",
            "https://user:secret@accounts.google.com/o/oauth2/v2/auth",
            "https://login.microsoftonline.com/common/redirect",
            "https://login.microsoftonline.com/common/oauth2/v2.0/authorize/",
            "https://accounts.google.com.evil/o/oauth2/v2/auth",
        ] {
            assert!(authorization_url(&json!(address)).is_err())
        }
    }
}
