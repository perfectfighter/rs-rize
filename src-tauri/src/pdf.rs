//! Invoice PDF export through the Windows system WebView2 printer.
//! The HTML is generated and escaped by rize-core, never supplied by web content.
use std::path::PathBuf;

#[cfg(not(windows))]
pub async fn export_pdf(
    _app: &tauri::AppHandle,
    _html: String,
    _path: PathBuf,
) -> Result<(), String> {
    Err("Invoice PDF export requires the Windows desktop application.".into())
}

#[cfg(windows)]
pub async fn export_pdf(app: &tauri::AppHandle, html: String, path: PathBuf) -> Result<(), String> {
    use std::{
        cell::Cell,
        io::Read,
        os::windows::ffi::OsStrExt,
        rc::Rc,
        sync::{Arc, Mutex},
        time::Duration,
    };
    use tauri::{WebviewUrl, WebviewWindowBuilder};
    use tokio::sync::oneshot;
    use webview2_com::{
        Microsoft::Web::WebView2::Win32::{
            ICoreWebView2Environment6, ICoreWebView2_7, COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL,
        },
        NavigationCompletedEventHandler, NavigationStartingEventHandler,
        PrintToPdfCompletedHandler, WebResourceRequestedEventHandler,
    };
    use windows::{
        core::{w, Interface, PCWSTR, PWSTR},
        Win32::System::Com::{CoTaskMemFree, IStream},
    };

    type Completion = Arc<Mutex<Option<oneshot::Sender<Result<(), String>>>>>;
    fn finish(sender: &Completion, result: Result<(), String>) {
        if let Ok(mut slot) = sender.lock() {
            if let Some(sender) = slot.take() {
                let _ = sender.send(result);
            }
        }
    }
    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
    }

    if html.len() > 8 * 1024 * 1024 || html.contains('\0') {
        return Err("The invoice is too large to render as a PDF.".into());
    }
    if !path.is_absolute()
        || path
            .extension()
            .is_none_or(|s| !s.eq_ignore_ascii_case("pdf"))
    {
        return Err("Choose an absolute PDF destination using the save dialog.".into());
    }
    let parent = path
        .parent()
        .filter(|p| p.is_dir())
        .ok_or("The selected PDF folder is unavailable.")?;
    // Print into a temporary sibling, so failures leave an existing invoice intact.
    let temporary = tempfile::Builder::new()
        .prefix(".rs-rize-invoice-")
        .suffix(".pdf")
        .tempfile_in(parent)
        .map_err(|_| "A temporary PDF could not be created in the selected folder.")?
        .into_temp_path();
    std::fs::remove_file(&temporary).map_err(|_| "The temporary PDF could not be prepared.")?;
    let output_path = temporary.to_path_buf();
    let sentinel = format!("rs-rize-pdf-{}", uuid::Uuid::new_v4());
    let label = sentinel.clone();
    // This is inserted before every content-bearing element and has no network source.
    let policy = format!("<meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; style-src 'unsafe-inline'; script-src 'none'; img-src data:; font-src 'none'; connect-src 'none'; base-uri 'none'; form-action 'none'\"><title>{sentinel}</title>");
    let html = if let Some(position) = html.find("<head>") {
        let (first, rest) = html.split_at(position + 6);
        format!("{first}{policy}{rest}")
    } else {
        format!("<!doctype html><html><head>{policy}</head><body>{html}</body></html>")
    };
    let window = WebviewWindowBuilder::new(
        app,
        &label,
        WebviewUrl::External("about:blank".parse().unwrap()),
    )
    .title("Export invoice")
    .visible(false)
    .focused(false)
    .skip_taskbar(true)
    .incognito(true)
    .inner_size(794., 1123.)
    .resizable(false)
    .disable_drag_drop_handler()
    .on_navigation(|url| url.as_str() == "about:blank")
    .build()
    .map_err(|_| "The Windows invoice renderer could not be opened.")?;
    let (sender, receiver) = oneshot::channel();
    let completion: Completion = Arc::new(Mutex::new(Some(sender)));
    let immediate = completion.clone();
    let dispatch=window.with_webview(move |platform| unsafe {
        let operation=(|| -> windows::core::Result<()> {
            let core=platform.controller().CoreWebView2()?;
            let environment=platform.environment();
            let settings=core.Settings()?;
            settings.SetIsScriptEnabled(false)?;settings.SetIsWebMessageEnabled(false)?;
            settings.SetAreHostObjectsAllowed(false)?;settings.SetAreDefaultScriptDialogsEnabled(false)?;
            settings.SetAreDevToolsEnabled(false)?;settings.SetAreDefaultContextMenusEnabled(false)?;
            core.Stop()?;
            // Deny all network resources independently of the document's CSP.
            core.AddWebResourceRequestedFilter(w!("*"),COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL)?;
            let response_environment=environment.clone();
            let blocker=WebResourceRequestedEventHandler::create(Box::new(move |_,args| {
                if let Some(args)=args {
                    let response=response_environment.CreateWebResourceResponse(None::<&IStream>,403,w!("Blocked"),w!("Content-Type: text/plain"))?;
                    args.SetResponse(&response)?;
                }
                Ok(())
            }));
            let mut token=0;core.add_WebResourceRequested(&blocker,&mut token)?;
            let print_settings=environment.cast::<ICoreWebView2Environment6>()?.CreatePrintSettings()?;
            print_settings.SetShouldPrintBackgrounds(true)?;print_settings.SetShouldPrintHeaderAndFooter(false)?;
            print_settings.SetPageWidth(8.2677165354)?;print_settings.SetPageHeight(11.6929133858)?;
            let expected_navigation=Rc::new(Cell::new(None));
            let navigation_id=expected_navigation.clone();
            let starting=NavigationStartingEventHandler::create(Box::new(move |_,args| {
                if let Some(args)=args {let mut id=0;args.NavigationId(&mut id)?;navigation_id.set(Some(id));}Ok(())
            }));
            let mut token=0;core.add_NavigationStarting(&starting,&mut token)?;
            let destination:Vec<u16>=output_path.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
            let navigation_completion=completion.clone();
            let mut started=false;
            let loaded=NavigationCompletedEventHandler::create(Box::new(move |sender,args| {
                if started { return Ok(()); }
                let (Some(sender),Some(args))=(sender,args) else { return Ok(()); };
                let mut navigation_id=0;args.NavigationId(&mut navigation_id)?;
                if expected_navigation.get()!=Some(navigation_id) {return Ok(());}
                // A pending initial about:blank completion must never print an empty page.
                let mut document_title=PWSTR::null();sender.DocumentTitle(&mut document_title)?;
                let actual_title=if document_title.is_null() {String::new()} else {let value=document_title.to_string().unwrap_or_default();CoTaskMemFree(Some(document_title.as_ptr().cast()));value};
                if actual_title!=sentinel { return Ok(()); }
                let mut success=windows::core::BOOL::default();args.IsSuccess(&mut success)?;
                if !success.as_bool() {finish(&navigation_completion,Err("The invoice could not be rendered. Try exporting it again.".into()));return Ok(());}
                started=true;
                let print_core=sender.cast::<ICoreWebView2_7>()?;
                let print_completion=navigation_completion.clone();
                let printed=PrintToPdfCompletedHandler::create(Box::new(move |result,succeeded| {
                    finish(&print_completion,if result.is_ok()&&succeeded {Ok(())} else {Err("The invoice PDF could not be saved. Check the destination folder and try again.".into())});Ok(())
                }));
                if print_core.PrintToPdf(PCWSTR(destination.as_ptr()),&print_settings,&printed).is_err() {
                    finish(&navigation_completion,Err("The Windows PDF printer could not start. Update WebView2 and try again.".into()));
                }
                Ok(())
            }));
            let mut token=0;core.add_NavigationCompleted(&loaded,&mut token)?;
            let document=wide(&html);core.NavigateToString(PCWSTR(document.as_ptr()))?;
            Ok(())
        })();
        if operation.is_err() { finish(&immediate,Err("The Windows invoice renderer is unavailable. Update WebView2 and try again.".into())); }
    });
    if dispatch.is_err() {
        let _ = window.close();
        return Err("The Windows invoice renderer could not be started.".into());
    }
    let result = match tokio::time::timeout(Duration::from_secs(60), receiver).await {
        Ok(Ok(result)) => result,
        Ok(Err(_)) => Err("The Windows invoice renderer closed before saving the PDF.".into()),
        Err(_) => Err("Invoice PDF export timed out. Try exporting it again.".into()),
    };
    let _ = window.close();
    result?;
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&temporary)
        .map_err(|_| "The invoice PDF was not written.")?;
    let mut header = [0u8; 5];
    file.read_exact(&mut header)
        .map_err(|_| "The invoice PDF is incomplete.")?;
    if &header != b"%PDF-" {
        return Err("The invoice renderer returned an invalid PDF.".into());
    }
    file.sync_all()
        .map_err(|_| "The invoice PDF could not be flushed to disk.")?;
    drop(file);
    temporary.persist(&path).map_err(|_| {
        "The invoice PDF could not replace the selected file. Close that file and try again."
    })?;
    Ok(())
}
