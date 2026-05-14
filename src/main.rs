#![windows_subsystem = "windows"]

use pulldown_cmark::{CodeBlockKind, Event as MdEvent, Options, Parser, Tag, TagEnd};
use std::cell::RefCell;
use std::env;
use std::fs;
use std::path::PathBuf;
use std::rc::Rc;
use std::thread;
use std::time::Duration;
use syntect::highlighting::ThemeSet;
use syntect::html::highlighted_html_for_string;
use syntect::parsing::SyntaxSet;
use tao::dpi::{LogicalSize, PhysicalPosition};
use tao::event::{Event as WinEvent, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoop, EventLoopBuilder, EventLoopProxy};
use tao::window::WindowBuilder;
use wry::WebViewBuilder;

#[derive(Debug, Clone)]
enum UserEvent {
    OpenFile(PathBuf),
}

struct Doc {
    id: u64,
    path: PathBuf,
    name: String,
    base_dir: String,
    markdown: String,
}

struct AppState {
    docs: Vec<Doc>,
    next_id: u64,
}

fn canonical_or_keep(path: &PathBuf) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.clone())
}

/// Result of opening a file: id and whether it was newly added (true) or already open (false).
struct OpenResult {
    id: u64,
    is_new: bool,
}

impl AppState {
    fn new() -> Self {
        Self {
            docs: Vec::new(),
            next_id: 1,
        }
    }

    fn add_from_path(&mut self, path: &PathBuf) -> Option<OpenResult> {
        let normalized = canonical_or_keep(path);
        for d in &self.docs {
            if canonical_or_keep(&d.path) == normalized {
                return Some(OpenResult { id: d.id, is_new: false });
            }
        }
        let markdown = fs::read_to_string(path).ok()?;
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "Untitled.md".to_string());
        let base_dir = path
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        let id = self.next_id;
        self.next_id += 1;
        self.docs.push(Doc {
            id,
            path: normalized,
            name,
            base_dir,
            markdown,
        });
        Some(OpenResult { id, is_new: true })
    }

    fn find(&self, id: u64) -> Option<&Doc> {
        self.docs.iter().find(|d| d.id == id)
    }

    fn update_markdown(&mut self, id: u64, markdown: String) {
        if let Some(d) = self.docs.iter_mut().find(|d| d.id == id) {
            d.markdown = markdown;
        }
    }

    fn replace_with_path(&mut self, id: u64, path: &PathBuf) -> bool {
        let markdown = match fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => return false,
        };
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "Untitled.md".to_string());
        let base_dir = path
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        let normalized = canonical_or_keep(path);
        if let Some(d) = self.docs.iter_mut().find(|d| d.id == id) {
            d.path = normalized;
            d.name = name;
            d.base_dir = base_dir;
            d.markdown = markdown;
            return true;
        }
        false
    }

    fn remove(&mut self, id: u64) {
        self.docs.retain(|d| d.id != id);
    }
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let initial_path: Option<PathBuf> = if args.len() > 1 {
        Some(PathBuf::from(&args[1]))
    } else {
        None
    };

    let (mutex_name, pipe_name) = instance_names();
    let is_primary = try_become_primary(&mutex_name).unwrap_or(true);

    if !is_primary {
        if let Some(ref p) = initial_path {
            forward_path_to_primary(&pipe_name, p);
        }
        return;
    }

    let mut state = AppState::new();
    let initial_id = if let Some(ref path) = initial_path {
        match state.add_from_path(path) {
            Some(r) => Some(r.id),
            None => {
                show_error(&format!("Failed to read file: {}", path.display()));
                return;
            }
        }
    } else {
        None
    };

    let initial_title = match initial_id.and_then(|id| state.find(id)) {
        Some(d) => d.name.clone(),
        None => "MD Viewer".to_string(),
    };

    let initial_html = render_shell_page(&state, initial_id);

    let event_loop: EventLoop<UserEvent> = EventLoopBuilder::<UserEvent>::with_user_event().build();
    {
        let proxy = event_loop.create_proxy();
        let pipe_name_owned = pipe_name.clone();
        thread::spawn(move || run_pipe_server(pipe_name_owned, proxy));
    }

    let monitor = event_loop
        .primary_monitor()
        .or_else(|| event_loop.available_monitors().next());

    let config_path = get_config_path();
    let (win_width, win_height) = load_window_geometry(&config_path);

    let mut window_builder = WindowBuilder::new()
        .with_title(format!("{} — MD Viewer", initial_title))
        .with_decorations(false)
        .with_inner_size(LogicalSize::new(win_width, win_height))
        .with_min_inner_size(LogicalSize::new(500.0, 400.0));

    if let Some(mon) = monitor {
        let size = mon.size();
        let pos = mon.position();
        let x = pos.x + (size.width as f64 / 2.0 - win_width / 2.0) as i32;
        let y = pos.y + (size.height as f64 / 2.0 - win_height / 2.0) as i32;
        window_builder = window_builder.with_position(PhysicalPosition::new(x, y));
    }

    let window = window_builder.build(&event_loop).unwrap();

    let hwnd = {
        use tao::platform::windows::WindowExtWindows;
        window.hwnd() as isize
    };

    // Allow edge resizing on borderless window
    unsafe {
        #[link(name = "user32")]
        extern "system" {
            fn GetWindowLongPtrW(hwnd: isize, index: i32) -> isize;
            fn SetWindowLongPtrW(hwnd: isize, index: i32, val: isize) -> isize;
            fn SetWindowPos(hwnd: isize, after: isize, x: i32, y: i32, w: i32, h: i32, flags: u32) -> i32;
        }
        let style = GetWindowLongPtrW(hwnd, -16);
        SetWindowLongPtrW(hwnd, -16, style | 0x00040000);
        SetWindowPos(hwnd, 0, 0, 0, 0, 0, 0x0027);
    }

    let html_content = if initial_html.len() > 1_800_000 {
        strip_large_data_uris(&initial_html)
    } else {
        initial_html
    };

    let state = Rc::new(RefCell::new(state));
    let webview: Rc<RefCell<Option<wry::WebView>>> = Rc::new(RefCell::new(None));
    let webview_for_ipc = webview.clone();
    let state_for_ipc = state.clone();
    let webview_for_dd = webview.clone();
    let state_for_dd = state.clone();
    let webview_for_loop = webview.clone();
    let state_for_loop = state.clone();

    let wv = WebViewBuilder::new()
        .with_html(&html_content)
        .with_ipc_handler(move |msg| {
            let body = msg.body().to_string();
            handle_ipc(&body, hwnd, &state_for_ipc, &webview_for_ipc);
        })
        .with_navigation_handler(move |uri| {
            if uri.starts_with("http://") || uri.starts_with("https://") {
                let _ = open::that(&uri);
                return false;
            }
            true
        })
        .with_drag_drop_handler(move |event| {
            if let wry::DragDropEvent::Drop { paths, .. } = event {
                for path in paths.iter() {
                    let is_md = path
                        .extension()
                        .map(|e| e == "md" || e == "markdown")
                        .unwrap_or(false);
                    if !is_md {
                        continue;
                    }
                    let path_buf = path.clone();
                    let script = {
                        let mut s = state_for_dd.borrow_mut();
                        match s.add_from_path(&path_buf) {
                            Some(r) if r.is_new => {
                                if let Some(doc) = s.find(r.id) {
                                    let html_body = render_markdown_body(&doc.markdown, &doc.base_dir);
                                    Some(format!(
                                        "window.mdv && mdv.addDoc({}, '{}', '{}', '{}', '{}', true);",
                                        r.id,
                                        base64_encode(doc.name.as_bytes()),
                                        base64_encode(doc.base_dir.as_bytes()),
                                        base64_encode(doc.markdown.as_bytes()),
                                        base64_encode(html_body.as_bytes()),
                                    ))
                                } else {
                                    None
                                }
                            }
                            Some(r) => Some(format!("window.mdv && mdv.switchTo({});", r.id)),
                            None => None,
                        }
                    };
                    if let Some(script) = script {
                        if let Some(wv) = webview_for_dd.borrow().as_ref() {
                            let _ = wv.evaluate_script(&script);
                        }
                    }
                }
            }
            true
        })
        .build(&window)
        .unwrap();

    *webview.borrow_mut() = Some(wv);

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        match event {
            WinEvent::UserEvent(UserEvent::OpenFile(path)) => {
                let script = {
                    let mut s = state_for_loop.borrow_mut();
                    match s.add_from_path(&path) {
                        Some(r) if r.is_new => {
                            if let Some(doc) = s.find(r.id) {
                                let html_body = render_markdown_body(&doc.markdown, &doc.base_dir);
                                Some(format!(
                                    "window.mdv && mdv.addDoc({}, '{}', '{}', '{}', '{}', true);",
                                    r.id,
                                    base64_encode(doc.name.as_bytes()),
                                    base64_encode(doc.base_dir.as_bytes()),
                                    base64_encode(doc.markdown.as_bytes()),
                                    base64_encode(html_body.as_bytes()),
                                ))
                            } else {
                                None
                            }
                        }
                        Some(r) => Some(format!("window.mdv && mdv.switchTo({});", r.id)),
                        None => None,
                    }
                };
                if let Some(script) = script {
                    if let Some(wv) = webview_for_loop.borrow().as_ref() {
                        let _ = wv.evaluate_script(&script);
                    }
                }
                unsafe {
                    #[link(name = "user32")]
                    extern "system" {
                        fn ShowWindow(hwnd: isize, cmd: i32) -> i32;
                        fn SetForegroundWindow(hwnd: isize) -> i32;
                        fn IsIconic(hwnd: isize) -> i32;
                    }
                    if IsIconic(hwnd) != 0 {
                        ShowWindow(hwnd, 9); // SW_RESTORE
                    }
                    SetForegroundWindow(hwnd);
                }
            }
            WinEvent::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => {
                // Route through JS so unsaved-changes prompt fires; JS posts 'force-close' when ready.
                if let Some(wv) = webview_for_loop.borrow().as_ref() {
                    let _ = wv.evaluate_script("window.mdv && mdv.tryCloseWindow();");
                } else {
                    terminate_self(hwnd);
                }
            }
            _ => {}
        }
    });
}

fn handle_ipc(
    body: &str,
    hwnd: isize,
    state: &Rc<RefCell<AppState>>,
    webview: &Rc<RefCell<Option<wry::WebView>>>,
) {
    unsafe {
        #[link(name = "user32")]
        extern "system" {
            fn ShowWindow(hwnd: isize, cmd: i32) -> i32;
            fn IsZoomed(hwnd: isize) -> i32;
            fn ReleaseCapture() -> i32;
            fn SendMessageW(hwnd: isize, msg: u32, wparam: usize, lparam: isize) -> isize;
        }
        match body {
            "minimize" => {
                ShowWindow(hwnd, 6);
                return;
            }
            "maximize" => {
                if IsZoomed(hwnd) != 0 {
                    ShowWindow(hwnd, 9);
                } else {
                    ShowWindow(hwnd, 3);
                }
                return;
            }
            "close" | "force-close" => {
                terminate_self(hwnd);
            }
            "drag" => {
                ReleaseCapture();
                SendMessageW(hwnd, 0x00A1, 2, 0);
                return;
            }
            _ => {}
        }
        if let Some(dir_str) = body.strip_prefix("resize:") {
            let dir: usize = match dir_str {
                "top" => 12,
                "bottom" => 15,
                "left" => 10,
                "right" => 11,
                "topleft" => 13,
                "topright" => 14,
                "bottomleft" => 16,
                "bottomright" => 17,
                _ => 0,
            };
            if dir != 0 {
                ReleaseCapture();
                SendMessageW(hwnd, 0x00A1, dir, 0);
            }
            return;
        }
    }

    if let Some(rest) = body.strip_prefix("render:") {
        if let Some((id_str, md_b64)) = rest.split_once(':') {
            if let Ok(id) = id_str.parse::<u64>() {
                if let Ok(md_bytes) = base64_decode(md_b64) {
                    if let Ok(markdown) = String::from_utf8(md_bytes) {
                        let base_dir = {
                            let s = state.borrow();
                            s.find(id).map(|d| d.base_dir.clone()).unwrap_or_default()
                        };
                        let html_body = render_markdown_body(&markdown, &base_dir);
                        {
                            let mut s = state.borrow_mut();
                            s.update_markdown(id, markdown);
                        }
                        let script = format!(
                            "window.mdv && mdv.applyRender({}, '{}');",
                            id,
                            base64_encode(html_body.as_bytes())
                        );
                        if let Some(wv) = webview.borrow().as_ref() {
                            let _ = wv.evaluate_script(&script);
                        }
                    }
                }
            }
        }
        return;
    }

    if let Some(id_str) = body.strip_prefix("close-tab:") {
        if let Ok(id) = id_str.parse::<u64>() {
            state.borrow_mut().remove(id);
        }
        return;
    }

    if let Some(rest) = body.strip_prefix("confirm-close-tab:") {
        if let Some((id_str, md_b64)) = rest.split_once(':') {
            if let Ok(id) = id_str.parse::<u64>() {
                if let Ok(md_bytes) = base64_decode(md_b64) {
                    if let Ok(markdown) = String::from_utf8(md_bytes) {
                        let info = {
                            let s = state.borrow();
                            s.find(id).map(|d| (d.name.clone(), d.path.clone()))
                        };
                        if let Some((name, path)) = info {
                            let text = format!("「{}」 有未保存的修改。\n\n是否保存？", name);
                            let answer = ask_save_dialog(hwnd, &text, "MD Viewer");
                            match answer {
                                6 => {
                                    // IDYES: save then close
                                    if fs::write(&path, &markdown).is_ok() {
                                        state.borrow_mut().update_markdown(id, markdown);
                                        if let Some(wv) = webview.borrow().as_ref() {
                                            let _ = wv.evaluate_script(&format!(
                                                "window.mdv && mdv.confirmCloseTab({});",
                                                id
                                            ));
                                        }
                                    } else if let Some(wv) = webview.borrow().as_ref() {
                                        let _ = wv.evaluate_script(&format!(
                                            "window.mdv && mdv.saveFailed({});",
                                            id
                                        ));
                                    }
                                }
                                7 => {
                                    // IDNO: close without saving
                                    if let Some(wv) = webview.borrow().as_ref() {
                                        let _ = wv.evaluate_script(&format!(
                                            "window.mdv && mdv.confirmCloseTab({});",
                                            id
                                        ));
                                    }
                                }
                                _ => {} // Cancel: do nothing
                            }
                        }
                    }
                }
            }
        }
        return;
    }

    if let Some(b64) = body.strip_prefix("confirm-close-window:") {
        let bytes = match base64_decode(b64) {
            Ok(b) => b,
            Err(_) => return,
        };
        let data = String::from_utf8(bytes).unwrap_or_default();
        let entries = parse_dirty_list(&data);
        let names: Vec<String> = {
            let s = state.borrow();
            entries
                .iter()
                .filter_map(|(id, _)| s.find(*id).map(|d| d.name.clone()))
                .collect()
        };
        let summary = if names.len() == 1 {
            format!("「{}」 有未保存的修改。\n\n是否保存？", names[0])
        } else {
            format!(
                "有 {} 个文件未保存：\n\n• {}\n\n是否全部保存？",
                names.len(),
                names.join("\n• ")
            )
        };
        let answer = ask_save_dialog(hwnd, &summary, "MD Viewer");
        match answer {
            6 => {
                // Save all
                let mut all_ok = true;
                let mut failed_id: Option<u64> = None;
                for (id, md) in &entries {
                    let path = state.borrow().find(*id).map(|d| d.path.clone());
                    if let Some(p) = path {
                        if fs::write(&p, md).is_ok() {
                            state.borrow_mut().update_markdown(*id, md.clone());
                        } else {
                            all_ok = false;
                            failed_id = Some(*id);
                            break;
                        }
                    }
                }
                if all_ok {
                    terminate_self(hwnd);
                } else if let Some(fid) = failed_id {
                    if let Some(wv) = webview.borrow().as_ref() {
                        let _ = wv.evaluate_script(&format!(
                            "window.mdv && mdv.saveFailed({});",
                            fid
                        ));
                    }
                }
            }
            7 => {
                terminate_self(hwnd);
            }
            _ => {} // Cancel
        }
        return;
    }

    if let Some(b64) = body.strip_prefix("list-md-files:") {
        let bytes = match base64_decode(b64) {
            Ok(b) => b,
            Err(_) => return,
        };
        let base_dir = String::from_utf8(bytes).unwrap_or_default();
        if base_dir.is_empty() {
            return;
        }
        let base_path = PathBuf::from(&base_dir);
        let mut files: Vec<String> = Vec::new();
        scan_md_files(&base_path, &base_path, &mut files, 0);
        files.sort();
        let list = files.join("\n");
        let script = format!(
            "window.mdv && mdv.applyFileTree('{}', '{}');",
            base64_encode(base_dir.as_bytes()),
            base64_encode(list.as_bytes())
        );
        if let Some(wv) = webview.borrow().as_ref() {
            let _ = wv.evaluate_script(&script);
        }
        return;
    }

    if let Some(b64) = body.strip_prefix("open-path-preview:") {
        let bytes = match base64_decode(b64) {
            Ok(b) => b,
            Err(_) => return,
        };
        let path_str = match String::from_utf8(bytes) {
            Ok(s) => s,
            Err(_) => return,
        };
        let path = PathBuf::from(path_str);
        let script = {
            let mut s = state.borrow_mut();
            match s.add_from_path(&path) {
                Some(r) if r.is_new => {
                    if let Some(doc) = s.find(r.id) {
                        let html_body = render_markdown_body(&doc.markdown, &doc.base_dir);
                        Some(format!(
                            "window.mdv && mdv.addDocPreview({}, '{}', '{}', '{}', '{}');",
                            r.id,
                            base64_encode(doc.name.as_bytes()),
                            base64_encode(doc.base_dir.as_bytes()),
                            base64_encode(doc.markdown.as_bytes()),
                            base64_encode(html_body.as_bytes()),
                        ))
                    } else {
                        None
                    }
                }
                Some(r) => Some(format!("window.mdv && mdv.switchTo({});", r.id)),
                None => None,
            }
        };
        if let Some(script) = script {
            if let Some(wv) = webview.borrow().as_ref() {
                let _ = wv.evaluate_script(&script);
            }
        }
        return;
    }

    if let Some(rest) = body.strip_prefix("replace-doc:") {
        if let Some((id_str, b64)) = rest.split_once(':') {
            if let Ok(id) = id_str.parse::<u64>() {
                if let Ok(bytes) = base64_decode(b64) {
                    if let Ok(path_str) = String::from_utf8(bytes) {
                        let path = PathBuf::from(path_str);
                        let script = {
                            let mut s = state.borrow_mut();
                            if s.replace_with_path(id, &path) {
                                s.find(id).map(|doc| {
                                    let html_body = render_markdown_body(&doc.markdown, &doc.base_dir);
                                    format!(
                                        "window.mdv && mdv.replaceDoc({}, '{}', '{}', '{}', '{}');",
                                        id,
                                        base64_encode(doc.name.as_bytes()),
                                        base64_encode(doc.base_dir.as_bytes()),
                                        base64_encode(doc.markdown.as_bytes()),
                                        base64_encode(html_body.as_bytes()),
                                    )
                                })
                            } else {
                                None
                            }
                        };
                        if let Some(script) = script {
                            if let Some(wv) = webview.borrow().as_ref() {
                                let _ = wv.evaluate_script(&script);
                            }
                        }
                    }
                }
            }
        }
        return;
    }

    if let Some(b64) = body.strip_prefix("open-path:") {
        let bytes = match base64_decode(b64) {
            Ok(b) => b,
            Err(_) => return,
        };
        let path_str = match String::from_utf8(bytes) {
            Ok(s) => s,
            Err(_) => return,
        };
        let path = PathBuf::from(path_str);
        let script = {
            let mut s = state.borrow_mut();
            match s.add_from_path(&path) {
                Some(r) if r.is_new => {
                    if let Some(doc) = s.find(r.id) {
                        let html_body = render_markdown_body(&doc.markdown, &doc.base_dir);
                        Some(format!(
                            "window.mdv && mdv.addDoc({}, '{}', '{}', '{}', '{}', true);",
                            r.id,
                            base64_encode(doc.name.as_bytes()),
                            base64_encode(doc.base_dir.as_bytes()),
                            base64_encode(doc.markdown.as_bytes()),
                            base64_encode(html_body.as_bytes()),
                        ))
                    } else {
                        None
                    }
                }
                Some(r) => Some(format!("window.mdv && mdv.switchTo({});", r.id)),
                None => None,
            }
        };
        if let Some(script) = script {
            if let Some(wv) = webview.borrow().as_ref() {
                let _ = wv.evaluate_script(&script);
            }
        }
        return;
    }

    if body == "open-dialog" {
        let paths = show_open_dialog(hwnd);
        for path in paths {
            let script = {
                let mut s = state.borrow_mut();
                match s.add_from_path(&path) {
                    Some(r) if r.is_new => {
                        if let Some(doc) = s.find(r.id) {
                            let html_body = render_markdown_body(&doc.markdown, &doc.base_dir);
                            Some(format!(
                                "window.mdv && mdv.addDoc({}, '{}', '{}', '{}', '{}', true);",
                                r.id,
                                base64_encode(doc.name.as_bytes()),
                                base64_encode(doc.base_dir.as_bytes()),
                                base64_encode(doc.markdown.as_bytes()),
                                base64_encode(html_body.as_bytes()),
                            ))
                        } else {
                            None
                        }
                    }
                    Some(r) => Some(format!("window.mdv && mdv.switchTo({});", r.id)),
                    None => None,
                }
            };
            if let Some(script) = script {
                if let Some(wv) = webview.borrow().as_ref() {
                    let _ = wv.evaluate_script(&script);
                }
            }
        }
        return;
    }

    if let Some(rest) = body.strip_prefix("paste-image:") {
        if let Some((id_str, b64)) = rest.split_once(':') {
            if let Ok(id) = id_str.parse::<u64>() {
                if let Ok(data) = base64_decode(b64) {
                    let ext = detect_image_ext(&data);
                    let base_dir_opt = {
                        let s = state.borrow();
                        s.find(id).map(|d| d.base_dir.clone())
                    };
                    if let Some(base_dir) = base_dir_opt {
                        if !base_dir.is_empty() {
                            let images_dir = PathBuf::from(&base_dir).join("images");
                            let _ = fs::create_dir_all(&images_dir);
                            let now = std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_millis();
                            let filename = format!("paste-{}.{}", now, ext);
                            let full = images_dir.join(&filename);
                            if fs::write(&full, &data).is_ok() {
                                let rel_path = format!("images/{}", filename);
                                let script = format!(
                                    "window.mdv && mdv.pasteImageInserted('{}');",
                                    base64_encode(rel_path.as_bytes())
                                );
                                if let Some(wv) = webview.borrow().as_ref() {
                                    let _ = wv.evaluate_script(&script);
                                }
                            }
                        }
                    }
                }
            }
        }
        return;
    }

    if let Some(rest) = body.strip_prefix("save:") {
        if let Some((id_str, md_b64)) = rest.split_once(':') {
            if let Ok(id) = id_str.parse::<u64>() {
                if let Ok(md_bytes) = base64_decode(md_b64) {
                    if let Ok(markdown) = String::from_utf8(md_bytes) {
                        let path_opt = {
                            let s = state.borrow();
                            s.find(id).map(|d| d.path.clone())
                        };
                        if let Some(path) = path_opt {
                            if fs::write(&path, &markdown).is_ok() {
                                state.borrow_mut().update_markdown(id, markdown);
                                let script = format!("window.mdv && mdv.markSaved({});", id);
                                if let Some(wv) = webview.borrow().as_ref() {
                                    let _ = wv.evaluate_script(&script);
                                }
                            } else {
                                let script = format!("window.mdv && mdv.saveFailed({});", id);
                                if let Some(wv) = webview.borrow().as_ref() {
                                    let _ = wv.evaluate_script(&script);
                                }
                            }
                        }
                    }
                }
            }
        }
        return;
    }
}

fn instance_names() -> (String, String) {
    let user = env::var("USERNAME").unwrap_or_else(|_| "default".to_string());
    let mutex = format!("Local\\md-viewer-singleton-{}", user);
    let pipe = format!("\\\\.\\pipe\\md-viewer-{}", user);
    (mutex, pipe)
}

/// Returns Some(true) if we became the primary instance (mutex created fresh),
/// Some(false) if another instance is already running, None on Win32 error.
fn try_become_primary(mutex_name: &str) -> Option<bool> {
    let wide: Vec<u16> = mutex_name
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    unsafe {
        #[link(name = "kernel32")]
        extern "system" {
            fn CreateMutexW(sa: *const u8, initial_owner: i32, name: *const u16) -> isize;
            fn GetLastError() -> u32;
        }
        let h = CreateMutexW(std::ptr::null(), 0, wide.as_ptr());
        if h == 0 {
            return None;
        }
        // Leak handle on purpose: lifetime tied to process.
        let err = GetLastError();
        // ERROR_ALREADY_EXISTS = 183
        Some(err != 183)
    }
}

fn forward_path_to_primary(pipe_name: &str, path: &PathBuf) -> bool {
    let abs = path
        .canonicalize()
        .unwrap_or_else(|_| path.clone())
        .to_string_lossy()
        .to_string();
    let wide: Vec<u16> = pipe_name.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        #[link(name = "kernel32")]
        extern "system" {
            fn WaitNamedPipeW(name: *const u16, timeout: u32) -> i32;
            fn CreateFileW(
                name: *const u16,
                access: u32,
                share: u32,
                sa: *const u8,
                disp: u32,
                attrs: u32,
                template: isize,
            ) -> isize;
            fn WriteFile(
                h: isize,
                buf: *const u8,
                bytes: u32,
                written: *mut u32,
                ovl: *const u8,
            ) -> i32;
            fn CloseHandle(h: isize) -> i32;
        }
        // Up to 3s for primary's pipe to be available.
        WaitNamedPipeW(wide.as_ptr(), 3000);
        let h = CreateFileW(
            wide.as_ptr(),
            0x4000_0000, // GENERIC_WRITE
            0,
            std::ptr::null(),
            3, // OPEN_EXISTING
            0,
            0,
        );
        if h == -1 || h == 0 {
            return false;
        }
        let bytes = abs.as_bytes();
        let mut written: u32 = 0;
        let ok = WriteFile(
            h,
            bytes.as_ptr(),
            bytes.len() as u32,
            &mut written,
            std::ptr::null(),
        );
        CloseHandle(h);
        ok != 0
    }
}

fn run_pipe_server(pipe_name: String, proxy: EventLoopProxy<UserEvent>) {
    let wide: Vec<u16> = pipe_name.encode_utf16().chain(std::iter::once(0)).collect();
    loop {
        let handle = unsafe {
            #[link(name = "kernel32")]
            extern "system" {
                fn CreateNamedPipeW(
                    name: *const u16,
                    open_mode: u32,
                    pipe_mode: u32,
                    max_inst: u32,
                    out_buf: u32,
                    in_buf: u32,
                    def_timeout: u32,
                    sa: *const u8,
                ) -> isize;
            }
            CreateNamedPipeW(
                wide.as_ptr(),
                0x0000_0001, // PIPE_ACCESS_INBOUND
                0,           // PIPE_TYPE_BYTE | PIPE_WAIT
                255,
                4096,
                4096,
                0,
                std::ptr::null(),
            )
        };
        if handle == -1 || handle == 0 {
            thread::sleep(Duration::from_millis(500));
            continue;
        }
        let connected = unsafe {
            #[link(name = "kernel32")]
            extern "system" {
                fn ConnectNamedPipe(h: isize, ovl: *const u8) -> i32;
            }
            ConnectNamedPipe(handle, std::ptr::null())
        };
        if connected != 0 {
            let mut total: Vec<u8> = Vec::new();
            let mut buf = vec![0u8; 4096];
            loop {
                let mut read: u32 = 0;
                let ok = unsafe {
                    #[link(name = "kernel32")]
                    extern "system" {
                        fn ReadFile(
                            h: isize,
                            buf: *mut u8,
                            bytes: u32,
                            read: *mut u32,
                            ovl: *const u8,
                        ) -> i32;
                    }
                    ReadFile(
                        handle,
                        buf.as_mut_ptr(),
                        buf.len() as u32,
                        &mut read,
                        std::ptr::null(),
                    )
                };
                if ok == 0 || read == 0 {
                    break;
                }
                total.extend_from_slice(&buf[..read as usize]);
                if (read as usize) < buf.len() {
                    break;
                }
            }
            if let Ok(s) = String::from_utf8(total) {
                let trimmed = s.trim();
                if !trimmed.is_empty() {
                    let _ = proxy.send_event(UserEvent::OpenFile(PathBuf::from(trimmed)));
                }
            }
        }
        unsafe {
            #[link(name = "kernel32")]
            extern "system" {
                fn DisconnectNamedPipe(h: isize) -> i32;
                fn CloseHandle(h: isize) -> i32;
            }
            DisconnectNamedPipe(handle);
            CloseHandle(handle);
        }
    }
}

#[repr(C)]
struct Ofnw {
    l_struct_size: u32,
    hwnd_owner: isize,
    h_instance: isize,
    lp_str_filter: *const u16,
    lp_str_custom_filter: *mut u16,
    n_max_cust_filter: u32,
    n_filter_index: u32,
    lp_str_file: *mut u16,
    n_max_file: u32,
    lp_str_file_title: *mut u16,
    n_max_file_title: u32,
    lp_str_initial_dir: *const u16,
    lp_str_title: *const u16,
    flags: u32,
    n_file_offset: u16,
    n_file_extension: u16,
    lp_str_def_ext: *const u16,
    l_cust_data: usize,
    lpfn_hook: usize,
    lp_template_name: *const u16,
    pv_reserved: *const u8,
    dw_reserved: u32,
    flags_ex: u32,
}

fn show_open_dialog(owner_hwnd: isize) -> Vec<PathBuf> {
    let mut buffer: Vec<u16> = vec![0u16; 32_768];

    // Each segment ends with NUL; the whole filter is terminated by an extra NUL.
    let mut filter: Vec<u16> = Vec::new();
    for part in [
        "Markdown Files (*.md;*.markdown)",
        "*.md;*.markdown",
        "All Files (*.*)",
        "*.*",
    ] {
        filter.extend(part.encode_utf16());
        filter.push(0);
    }
    filter.push(0);

    let title: Vec<u16> = "Open Markdown File"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();

    let mut ofn = Ofnw {
        l_struct_size: std::mem::size_of::<Ofnw>() as u32,
        hwnd_owner: owner_hwnd,
        h_instance: 0,
        lp_str_filter: filter.as_ptr(),
        lp_str_custom_filter: std::ptr::null_mut(),
        n_max_cust_filter: 0,
        n_filter_index: 1,
        lp_str_file: buffer.as_mut_ptr(),
        n_max_file: buffer.len() as u32,
        lp_str_file_title: std::ptr::null_mut(),
        n_max_file_title: 0,
        lp_str_initial_dir: std::ptr::null(),
        lp_str_title: title.as_ptr(),
        // OFN_EXPLORER | OFN_ALLOWMULTISELECT | OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR
        flags: 0x0008_0000 | 0x0000_0200 | 0x0000_1000 | 0x0000_0800 | 0x0000_0008,
        n_file_offset: 0,
        n_file_extension: 0,
        lp_str_def_ext: std::ptr::null(),
        l_cust_data: 0,
        lpfn_hook: 0,
        lp_template_name: std::ptr::null(),
        pv_reserved: std::ptr::null(),
        dw_reserved: 0,
        flags_ex: 0,
    };

    let ok = unsafe {
        #[link(name = "comdlg32")]
        extern "system" {
            fn GetOpenFileNameW(ofn: *mut Ofnw) -> i32;
        }
        GetOpenFileNameW(&mut ofn)
    };

    if ok == 0 {
        return Vec::new();
    }

    // With OFN_EXPLORER + OFN_ALLOWMULTISELECT:
    // - one file: buffer = "C:\\full\\path\0"
    // - multi: buffer = "C:\\dir\0name1\0name2\0...\0\0"
    let mut segments: Vec<String> = Vec::new();
    let mut start = 0usize;
    for i in 0..buffer.len() {
        if buffer[i] == 0 {
            if i == start {
                break;
            }
            segments.push(String::from_utf16_lossy(&buffer[start..i]));
            start = i + 1;
        }
    }

    if segments.is_empty() {
        return Vec::new();
    }
    if segments.len() == 1 {
        return vec![PathBuf::from(&segments[0])];
    }
    let dir = PathBuf::from(&segments[0]);
    segments[1..].iter().map(|n| dir.join(n)).collect()
}

fn detect_image_ext(data: &[u8]) -> &'static str {
    if data.starts_with(&[0x89, 0x50, 0x4E, 0x47]) {
        "png"
    } else if data.starts_with(&[0xFF, 0xD8, 0xFF]) {
        "jpg"
    } else if data.starts_with(b"GIF8") {
        "gif"
    } else if data.starts_with(b"RIFF") && data.len() > 11 && &data[8..12] == b"WEBP" {
        "webp"
    } else if data.starts_with(b"BM") {
        "bmp"
    } else {
        "png"
    }
}

fn scan_md_files(dir: &std::path::Path, base: &std::path::Path, out: &mut Vec<String>, depth: u32) {
    if depth > 12 || out.len() > 5000 {
        return;
    }
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(s) => s.to_string(),
            None => continue,
        };
        if name.starts_with('.') {
            continue;
        }
        if path.is_dir() {
            if matches!(
                name.as_str(),
                "node_modules" | "target" | "dist" | "build" | "out" | ".git" | "__pycache__"
            ) {
                continue;
            }
            scan_md_files(&path, base, out, depth + 1);
        } else if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            let ext_lc = ext.to_ascii_lowercase();
            if ext_lc == "md" || ext_lc == "markdown" {
                if let Ok(rel) = path.strip_prefix(base) {
                    out.push(rel.to_string_lossy().replace('\\', "/"));
                }
            }
        }
    }
}

fn ask_save_dialog(owner: isize, text: &str, caption: &str) -> i32 {
    let wide_text: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let wide_caption: Vec<u16> = caption.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        #[link(name = "user32")]
        extern "system" {
            fn MessageBoxW(hwnd: isize, text: *const u16, caption: *const u16, typ: u32) -> i32;
        }
        // MB_YESNOCANCEL(3) | MB_ICONWARNING(0x30) | MB_TOPMOST(0x40000)
        MessageBoxW(
            owner,
            wide_text.as_ptr(),
            wide_caption.as_ptr(),
            3 | 0x30 | 0x4_0000,
        )
    }
}

fn parse_dirty_list(data: &str) -> Vec<(u64, String)> {
    let mut out = Vec::new();
    for line in data.lines() {
        if let Some(sp) = line.find(' ') {
            let id_str = &line[..sp];
            let md_b64 = &line[sp + 1..];
            if let Ok(id) = id_str.parse::<u64>() {
                if let Ok(bytes) = base64_decode(md_b64) {
                    if let Ok(md) = String::from_utf8(bytes) {
                        out.push((id, md));
                    }
                }
            }
        }
    }
    out
}

fn terminate_self(hwnd: isize) -> ! {
    save_window_geometry_from_hwnd(hwnd);
    unsafe {
        #[link(name = "kernel32")]
        extern "system" {
            fn GetCurrentProcess() -> isize;
            fn TerminateProcess(handle: isize, code: u32) -> i32;
        }
        TerminateProcess(GetCurrentProcess(), 0);
    }
    // Unreachable, but the compiler doesn't know.
    std::process::exit(0);
}

fn show_error(msg: &str) {
    let wide_msg: Vec<u16> = msg.encode_utf16().chain(std::iter::once(0)).collect();
    let wide_title: Vec<u16> = "MD Viewer"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    unsafe {
        #[link(name = "user32")]
        extern "system" {
            fn MessageBoxW(hwnd: isize, text: *const u16, caption: *const u16, typ: u32) -> i32;
        }
        MessageBoxW(0, wide_msg.as_ptr(), wide_title.as_ptr(), 0x10);
    }
}

fn get_config_path() -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push("md-viewer");
    let _ = fs::create_dir_all(&p);
    p.push("window.conf");
    p
}

fn load_window_geometry(path: &PathBuf) -> (f64, f64) {
    let default = (1100.0, 800.0);
    let content = match fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return default,
    };
    let mut w = 1100.0f64;
    let mut h = 800.0f64;
    for line in content.lines() {
        let parts: Vec<&str> = line.splitn(2, '=').collect();
        if parts.len() != 2 {
            continue;
        }
        match parts[0].trim() {
            "width" => w = parts[1].trim().parse().unwrap_or(1100.0),
            "height" => h = parts[1].trim().parse().unwrap_or(800.0),
            _ => {}
        }
    }
    if w < 500.0 {
        w = 500.0;
    }
    if h < 400.0 {
        h = 400.0;
    }
    if w > 4000.0 {
        w = 1100.0;
    }
    if h > 3000.0 {
        h = 800.0;
    }
    (w, h)
}

fn save_window_geometry_from_hwnd(hwnd: isize) {
    unsafe {
        #[repr(C)]
        struct Rect {
            left: i32,
            top: i32,
            right: i32,
            bottom: i32,
        }
        #[link(name = "user32")]
        extern "system" {
            fn GetWindowRect(hwnd: isize, rect: *mut Rect) -> i32;
            fn IsZoomed(hwnd: isize) -> i32;
        }
        if IsZoomed(hwnd) != 0 {
            return;
        }
        let mut rc = Rect {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        if GetWindowRect(hwnd, &mut rc) != 0 {
            let w = rc.right - rc.left;
            let h = rc.bottom - rc.top;
            let content = format!("width={}\nheight={}\n", w, h);
            let _ = fs::write(get_config_path(), content);
        }
    }
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn image_to_data_uri(src: &str, base_dir: &str) -> Option<String> {
    use std::path::Path;

    if src.starts_with("http://") || src.starts_with("https://") || src.starts_with("data:") {
        return None;
    }

    let path = if Path::new(src).is_absolute() {
        PathBuf::from(src)
    } else {
        PathBuf::from(base_dir).join(src)
    };

    let data = fs::read(&path).ok()?;
    let mime = match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .as_deref()
    {
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("svg") => "image/svg+xml",
        Some("webp") => "image/webp",
        Some("bmp") => "image/bmp",
        _ => "application/octet-stream",
    };

    use std::fmt::Write;
    let mut b64 = String::new();
    let encoded = base64_encode(&data);
    let _ = write!(b64, "data:{};base64,{}", mime, encoded);
    Some(b64)
}

fn base64_encode(data: &[u8]) -> String {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut result = String::with_capacity((data.len() + 2) / 3 * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let n = (b0 << 16) | (b1 << 8) | b2;
        result.push(CHARS[((n >> 18) & 63) as usize] as char);
        result.push(CHARS[((n >> 12) & 63) as usize] as char);
        if chunk.len() > 1 {
            result.push(CHARS[((n >> 6) & 63) as usize] as char);
        } else {
            result.push('=');
        }
        if chunk.len() > 2 {
            result.push(CHARS[(n & 63) as usize] as char);
        } else {
            result.push('=');
        }
    }
    result
}

fn base64_decode(s: &str) -> Result<Vec<u8>, &'static str> {
    let cleaned: Vec<u8> = s.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    if cleaned.is_empty() {
        return Ok(Vec::new());
    }
    if cleaned.len() % 4 != 0 {
        return Err("base64 length");
    }
    let dec = |c: u8| -> Result<i16, &'static str> {
        match c {
            b'A'..=b'Z' => Ok((c - b'A') as i16),
            b'a'..=b'z' => Ok((c - b'a' + 26) as i16),
            b'0'..=b'9' => Ok((c - b'0' + 52) as i16),
            b'+' | b'-' => Ok(62),
            b'/' | b'_' => Ok(63),
            b'=' => Ok(-1),
            _ => Err("base64 char"),
        }
    };
    let mut out = Vec::with_capacity(cleaned.len() / 4 * 3);
    for chunk in cleaned.chunks(4) {
        let a = dec(chunk[0])?;
        let b = dec(chunk[1])?;
        let c = dec(chunk[2])?;
        let d = dec(chunk[3])?;
        if a < 0 || b < 0 {
            return Err("base64 prefix");
        }
        let n = ((a as u32) << 18)
            | ((b as u32) << 12)
            | ((c.max(0) as u32) << 6)
            | (d.max(0) as u32);
        out.push(((n >> 16) & 0xff) as u8);
        if c >= 0 {
            out.push(((n >> 8) & 0xff) as u8);
        }
        if d >= 0 {
            out.push((n & 0xff) as u8);
        }
    }
    Ok(out)
}

fn strip_large_data_uris(html: &str) -> String {
    let mut result = String::with_capacity(html.len() / 2);
    let mut pos = 0;
    while pos < html.len() {
        if let Some(idx) = html[pos..].find("data:image") {
            let abs = pos + idx;
            if let Some(end) = html[abs..].find('"').or_else(|| html[abs..].find('\'')) {
                let uri = &html[abs..abs + end];
                if uri.len() > 200_000 {
                    result.push_str(&html[pos..abs]);
                    pos = abs + end;
                    continue;
                }
            }
        }
        result.push_str(&html[pos..]);
        break;
    }
    result
}

fn embed_local_images(html: &str, base_dir: &str) -> String {
    let mut result = String::with_capacity(html.len());
    let mut pos = 0;

    while pos < html.len() {
        let search = &html[pos..];
        let img_idx = search.find("<img ").or_else(|| search.find("<img\n"));
        if img_idx.is_none() {
            result.push_str(&html[pos..]);
            break;
        }
        let img_abs = pos + img_idx.unwrap();
        let tag_end = match html[img_abs..].find('>') {
            Some(e) => e,
            None => {
                result.push_str(&html[pos..]);
                break;
            }
        };
        let tag = &html[img_abs..img_abs + tag_end + 1];
        if let Some(replaced_tag) = replace_img_src(tag, base_dir) {
            result.push_str(&html[pos..img_abs]);
            result.push_str(&replaced_tag);
        } else {
            result.push_str(&html[pos..img_abs + tag_end + 1]);
        }
        pos = img_abs + tag_end + 1;
    }
    result
}

fn replace_img_src(tag: &str, base_dir: &str) -> Option<String> {
    let src_pos = tag.find("src=\"").or_else(|| tag.find("src='"))?;
    let quote = tag.as_bytes()[src_pos + 4] as char;
    let val_start = src_pos + 5;
    let val_end = tag[val_start..].find(quote)? + val_start;
    let src_val = &tag[val_start..val_end];

    if src_val.starts_with("http://") || src_val.starts_with("https://") || src_val.starts_with("data:") {
        return None;
    }

    let data_uri = image_to_data_uri(src_val, base_dir)?;
    let mut new_tag = String::new();
    new_tag.push_str(&tag[..val_start]);
    new_tag.push_str(&data_uri);
    new_tag.push_str(&tag[val_end..]);
    Some(new_tag)
}

fn is_block_start_tag(tag: &Tag) -> bool {
    matches!(
        tag,
        Tag::Paragraph
            | Tag::Heading { .. }
            | Tag::BlockQuote(..)
            | Tag::CodeBlock(_)
            | Tag::List(_)
            | Tag::FootnoteDefinition(_)
            | Tag::Table(_)
            | Tag::HtmlBlock
            | Tag::MetadataBlock(_)
    )
}

fn is_block_end_tag(tag: &TagEnd) -> bool {
    matches!(
        tag,
        TagEnd::Paragraph
            | TagEnd::Heading(_)
            | TagEnd::BlockQuote(..)
            | TagEnd::CodeBlock
            | TagEnd::List(_)
            | TagEnd::FootnoteDefinition
            | TagEnd::Table
            | TagEnd::HtmlBlock
            | TagEnd::MetadataBlock(_)
    )
}

fn render_markdown_body(markdown: &str, base_dir: &str) -> String {
    let mut options = Options::all();
    options.remove(Options::ENABLE_SMART_PUNCTUATION);

    // Byte-offset -> line index, for tagging each top-level block with its source line.
    let line_starts: Vec<usize> = {
        let mut v = vec![0usize];
        for (i, b) in markdown.bytes().enumerate() {
            if b == b'\n' {
                v.push(i + 1);
            }
        }
        v
    };
    let byte_to_line = |offset: usize| -> usize {
        match line_starts.binary_search(&offset) {
            Ok(i) => i,
            Err(i) => i.saturating_sub(1),
        }
    };

    let ss = SyntaxSet::load_defaults_newlines();
    let ts = ThemeSet::load_defaults();
    let theme = &ts.themes["base16-ocean.dark"];

    let parser = Parser::new_ext(markdown, options).into_offset_iter();
    let mut html_body = String::new();
    let mut in_code_block = false;
    let mut code_lang = String::new();
    let mut code_text = String::new();
    let mut in_image = false;
    let mut block_depth: u32 = 0;

    for (event, range) in parser {
        // Wrap top-level block starts with <div class="md-block" data-md-line="N">
        if let MdEvent::Start(ref tag) = event {
            if is_block_start_tag(tag) {
                if block_depth == 0 {
                    let line = byte_to_line(range.start);
                    html_body.push_str(&format!(
                        "<div class=\"md-block\" data-md-line=\"{}\">",
                        line
                    ));
                }
                block_depth += 1;
                if let Tag::CodeBlock(kind) = tag {
                    in_code_block = true;
                    code_text.clear();
                    code_lang = match kind {
                        CodeBlockKind::Fenced(lang) => lang.to_string(),
                        _ => String::new(),
                    };
                    continue;
                }
                pulldown_cmark::html::push_html(&mut html_body, std::iter::once(event));
                continue;
            }
        }

        // Close <div> after top-level block end.
        if let MdEvent::End(ref tag_end) = event {
            if is_block_end_tag(tag_end) {
                if matches!(tag_end, TagEnd::CodeBlock) {
                    in_code_block = false;
                    let lang_token = if code_lang.is_empty() { "txt" } else { &code_lang };
                    let syntax = ss
                        .find_syntax_by_token(lang_token)
                        .unwrap_or_else(|| ss.find_syntax_plain_text());
                    let highlighted = highlighted_html_for_string(&code_text, &ss, syntax, theme)
                        .unwrap_or_else(|_| {
                            format!("<pre><code>{}</code></pre>", html_escape(&code_text))
                        });
                    html_body.push_str(&format!(
                        "<div class=\"syntect-block\" data-lang=\"{lang}\">{highlighted}</div>",
                        lang = html_escape(&code_lang),
                        highlighted = highlighted
                    ));
                } else {
                    pulldown_cmark::html::push_html(&mut html_body, std::iter::once(event));
                }
                block_depth = block_depth.saturating_sub(1);
                if block_depth == 0 {
                    html_body.push_str("</div>");
                }
                continue;
            }
        }

        // Inline / other events
        match event {
            MdEvent::Text(text) if in_code_block => {
                code_text.push_str(&text);
            }
            MdEvent::Start(Tag::Image { dest_url, title, .. }) => {
                let src = match image_to_data_uri(&dest_url, base_dir) {
                    Some(data_uri) => data_uri,
                    None => dest_url.to_string(),
                };
                html_body.push_str(&format!("<img src=\"{}\" alt=\"", html_escape(&src)));
                if !title.is_empty() {
                    html_body.push_str("\" title=\"");
                    html_body.push_str(&html_escape(&title));
                }
                in_image = true;
            }
            MdEvent::Text(text) if in_image => {
                html_body.push_str(&html_escape(&text));
            }
            MdEvent::End(TagEnd::Image) => {
                html_body.push_str("\" />");
                in_image = false;
            }
            MdEvent::Rule => {
                let line = byte_to_line(range.start);
                html_body.push_str(&format!(
                    "<hr class=\"md-block\" data-md-line=\"{}\" />",
                    line
                ));
            }
            other => {
                pulldown_cmark::html::push_html(&mut html_body, std::iter::once(other));
            }
        }
    }

    embed_local_images(&html_body, base_dir)
}

fn render_shell_page(state: &AppState, active_id: Option<u64>) -> String {
    let mut docs_js = String::from("[");
    for (i, doc) in state.docs.iter().enumerate() {
        if i > 0 {
            docs_js.push(',');
        }
        let html_body = render_markdown_body(&doc.markdown, &doc.base_dir);
        docs_js.push_str(&format!(
            "{{id:{},name:'{}',baseDir:'{}',markdown:'{}',htmlBody:'{}'}}",
            doc.id,
            base64_encode(doc.name.as_bytes()),
            base64_encode(doc.base_dir.as_bytes()),
            base64_encode(doc.markdown.as_bytes()),
            base64_encode(html_body.as_bytes()),
        ));
    }
    docs_js.push(']');

    let active_js = match active_id {
        Some(id) => id.to_string(),
        None => "null".to_string(),
    };

    let ver = env!("CARGO_PKG_VERSION");

    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<base id="docBase" href="">
<title>MD Viewer</title>
<style>
:root {{
  --bg: #ffffff;
  --fg: #1a1a2e;
  --fg-secondary: #555770;
  --border: #e2e4f0;
  --accent: #4361ee;
  --accent-light: #eef1ff;
  --code-bg: #f6f8fc;
  --block-bg: #f8f9fd;
  --shadow: 0 1px 3px rgba(0,0,0,.06);
  --radius: 8px;
  --titlebar-bg: #f0f1f8;
  --titlebar-border: #dfe1ed;
  --tab-bg: transparent;
  --tab-hover: rgba(0,0,0,.05);
  --tab-active-bg: #ffffff;
  --tab-active-border: #4361ee;
  --btn-hover: rgba(0,0,0,.07);
  --btn-close-hover: #e81123;
  --editor-bg: #fbfbff;
  --drop-bg: #f8f9fd;
}}

@media (prefers-color-scheme: dark) {{
  :root {{
    --bg: #16161e;
    --fg: #e4e5f1;
    --fg-secondary: #9b9cb8;
    --border: #2a2b3d;
    --accent: #7b93f5;
    --accent-light: #1e2140;
    --code-bg: #1e1f2e;
    --block-bg: #1a1b2a;
    --shadow: 0 1px 3px rgba(0,0,0,.3);
    --titlebar-bg: #1a1b26;
    --titlebar-border: #24253a;
    --tab-bg: transparent;
    --tab-hover: rgba(255,255,255,.06);
    --tab-active-bg: #16161e;
    --tab-active-border: #7b93f5;
    --btn-hover: rgba(255,255,255,.08);
    --btn-close-hover: #e81123;
    --editor-bg: #14141c;
    --drop-bg: #1a1b2a;
  }}
}}

* {{ margin: 0; padding: 0; box-sizing: border-box; }}

html {{
  font-size: 16px;
  scroll-behavior: smooth;
  -webkit-font-smoothing: antialiased;
  overflow: hidden;
  height: 100%;
}}

body {{
  font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, "Noto Sans SC", "Microsoft YaHei", sans-serif;
  background: var(--bg);
  color: var(--fg);
  line-height: 1.75;
  height: 100%;
  display: flex;
  flex-direction: column;
  overflow: hidden;
}}

/* ===== Title Bar ===== */
.titlebar {{
  flex-shrink: 0;
  height: 38px;
  background: var(--titlebar-bg);
  border-bottom: 1px solid var(--titlebar-border);
  display: flex;
  align-items: stretch;
  z-index: 9999;
  user-select: none;
  -app-region: drag;
}}
.titlebar-icon {{
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 0 12px 0 14px;
  flex-shrink: 0;
  pointer-events: none;
}}
.titlebar-img {{ width: 16px; height: 16px; flex-shrink: 0; }}
.titlebar-brand {{
  font-size: 12.5px;
  font-weight: 500;
  color: var(--fg-secondary);
  letter-spacing: 0.01em;
  white-space: nowrap;
}}
.titlebar-ver {{ font-size: 10px; opacity: 0.5; font-weight: 400; }}

/* ===== Tab bar (own row below titlebar) ===== */
.tab-bar {{
  flex-shrink: 0;
  height: 34px;
  display: flex;
  align-items: stretch;
  background: var(--titlebar-bg);
  border-bottom: 1px solid var(--titlebar-border);
  overflow-x: auto;
  overflow-y: hidden;
  scrollbar-width: none;
  -ms-overflow-style: none;
}}
.tab-bar::-webkit-scrollbar {{ height: 0; display: none; }}
.tab-bar.empty {{ display: none; }}
.tab {{
  flex-shrink: 0;
  min-width: 100px;
  max-width: 240px;
  height: 100%;
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 0 8px 0 14px;
  background: var(--tab-bg);
  border-right: 1px solid var(--titlebar-border);
  cursor: pointer;
  transition: background .12s;
  position: relative;
  color: var(--fg-secondary);
}}
.tab:hover {{ background: var(--tab-hover); color: var(--fg); }}
.tab.active {{
  background: var(--bg);
  color: var(--fg);
}}
.tab.active::after {{
  content: '';
  position: absolute;
  left: 0; right: 0; bottom: -1px;
  height: 2px;
  background: var(--tab-active-border);
}}
.tab-label {{
  font-size: 12.5px;
  font-weight: 500;
  flex: 1;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}}
.tab-close {{
  width: 18px;
  height: 18px;
  border: none;
  background: transparent;
  border-radius: 4px;
  cursor: pointer;
  display: flex;
  align-items: center;
  justify-content: center;
  flex-shrink: 0;
  opacity: 0.55;
  transition: background .12s, opacity .12s;
}}
.tab-close:hover {{
  background: rgba(0,0,0,.12);
  opacity: 1;
}}
.tab-close svg {{ width: 9px; height: 9px; stroke: currentColor; fill: none; stroke-width: 1.5; stroke-linecap: round; }}
@media (prefers-color-scheme: dark) {{
  .tab-close:hover {{ background: rgba(255,255,255,.12); }}
}}

/* ===== Mode toggle (floating top-right of content area) ===== */
.mode-group {{
  position: absolute;
  top: 10px;
  right: 14px;
  z-index: 20;
  display: flex;
  align-items: center;
  padding: 4px;
  gap: 2px;
  background: var(--titlebar-bg);
  border: 1px solid var(--border);
  border-radius: 8px;
  box-shadow: 0 2px 8px rgba(0,0,0,.08);
  backdrop-filter: blur(4px);
}}
@media (prefers-color-scheme: dark) {{
  .mode-group {{ box-shadow: 0 2px 8px rgba(0,0,0,.4); }}
}}
.mode-btn {{
  height: 24px;
  padding: 0 9px;
  border: 1px solid transparent;
  background: transparent;
  cursor: pointer;
  border-radius: 5px;
  display: flex;
  align-items: center;
  gap: 5px;
  font-size: 12px;
  font-weight: 500;
  color: var(--fg-secondary);
  transition: background .12s, color .12s, border-color .12s;
  white-space: nowrap;
}}
.mode-btn svg {{ width: 12px; height: 12px; fill: none; stroke: currentColor; stroke-width: 1.6; stroke-linecap: round; stroke-linejoin: round; }}
.mode-btn:hover {{
  background: var(--btn-hover);
  color: var(--fg);
}}
.mode-btn.active {{
  background: var(--accent-light);
  color: var(--accent);
  border-color: var(--accent);
}}
.mode-group.disabled {{ display: none; }}

/* ===== Window controls ===== */
.titlebar-controls {{
  display: flex;
  height: 100%;
  flex-shrink: 0;
  margin-left: auto;
}}
.titlebar-btn {{
  width: 46px;
  height: 100%;
  border: none;
  background: transparent;
  cursor: pointer;
  display: flex;
  align-items: center;
  justify-content: center;
  transition: background .12s;
}}
.titlebar-btn svg {{
  width: 10px;
  height: 10px;
  fill: none;
  stroke: var(--fg-secondary);
  stroke-width: 1.2;
  stroke-linecap: round;
}}
.titlebar-btn:hover {{ background: var(--btn-hover); }}
.titlebar-btn:hover svg {{ stroke: var(--fg); }}
.titlebar-btn.close:hover {{ background: var(--btn-close-hover); }}
.titlebar-btn.close:hover svg {{ stroke: #fff; }}

/* ===== Content area ===== */
.content-area {{
  flex: 1;
  display: flex;
  min-height: 0;
  margin: 0 8px 8px 8px;
  position: relative;
}}

/* TOC sidebar (view mode only) */
.toc-sidebar {{
  position: relative;
  width: 240px;
  min-width: 180px;
  max-width: 480px;
  flex-shrink: 0;
  background: var(--block-bg);
  border-right: 1px solid var(--border);
  display: flex;
  flex-direction: column;
  overflow: hidden;
}}
.toc-sidebar.collapsed {{ display: none; }}

.sidebar-tabs {{
  display: flex;
  gap: 4px;
  padding: 8px 8px 6px;
  flex-shrink: 0;
  border-bottom: 1px solid var(--border);
}}
.sidebar-tab {{
  flex: 1;
  height: 28px;
  border: 1px solid transparent;
  background: transparent;
  cursor: pointer;
  color: var(--fg-secondary);
  font-size: 12px;
  font-weight: 600;
  font-family: inherit;
  border-radius: 5px;
  transition: background .12s, color .12s, border-color .12s;
}}
.sidebar-tab:hover {{ background: var(--btn-hover); color: var(--fg); }}
.sidebar-tab.active {{
  background: var(--accent-light);
  color: var(--accent);
  border-color: var(--accent);
}}

.sidebar-pane {{
  flex: 1;
  display: flex;
  flex-direction: column;
  overflow: hidden;
  min-height: 0;
}}
.sidebar-pane[hidden] {{ display: none; }}

.files-search {{
  padding: 8px 10px;
  border-bottom: 1px solid var(--border);
  flex-shrink: 0;
}}
.files-search input {{
  width: 100%;
  height: 28px;
  padding: 0 8px;
  border: 1px solid var(--border);
  border-radius: 4px;
  background: var(--bg);
  color: var(--fg);
  font-size: 12px;
  font-family: inherit;
  outline: none;
  transition: border-color .12s;
}}
.files-search input:focus {{ border-color: var(--accent); }}

.files-tree {{
  flex: 1;
  overflow-y: auto;
  overflow-x: hidden;
  padding: 4px 0 16px;
  font-size: 12.5px;
}}
.files-tree::-webkit-scrollbar {{ width: 8px; }}
.files-tree::-webkit-scrollbar-track {{ background: transparent; }}
.files-tree::-webkit-scrollbar-thumb {{ background: var(--border); border-radius: 4px; }}
.files-tree::-webkit-scrollbar-thumb:hover {{ background: var(--fg-secondary); }}

.tree-item {{
  display: flex;
  align-items: center;
  gap: 4px;
  padding: 3px 8px 3px 4px;
  cursor: pointer;
  color: var(--fg-secondary);
  user-select: none;
  white-space: nowrap;
  line-height: 1.4;
  transition: background .1s, color .1s;
}}
.tree-item:hover {{ background: var(--accent-light); color: var(--fg); }}
.tree-item.preview-active:not(.active) {{
  background: rgba(127, 127, 127, 0.22);
  color: var(--fg);
}}
@media (prefers-color-scheme: dark) {{
  .tree-item.preview-active:not(.active) {{
    background: rgba(255, 255, 255, 0.12);
  }}
}}
.tree-item.active {{
  background: var(--accent-light);
  color: var(--accent);
  font-weight: 600;
}}
.tree-item .chevron {{
  width: 12px;
  height: 12px;
  flex-shrink: 0;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  transition: transform .15s;
}}
.tree-item .chevron svg {{
  width: 8px; height: 8px;
  fill: none; stroke: currentColor; stroke-width: 2;
  stroke-linecap: round; stroke-linejoin: round;
}}
.tree-folder:not(.collapsed) > .tree-item .chevron {{ transform: rotate(90deg); }}
.tree-folder.collapsed > .tree-children {{ display: none; }}
.tree-children {{ margin: 0; padding: 0; list-style: none; }}
.tree-icon {{
  width: 14px;
  height: 14px;
  flex-shrink: 0;
  display: inline-flex;
  align-items: center;
  justify-content: center;
}}
.tree-icon svg {{
  width: 14px; height: 14px;
  fill: none; stroke: currentColor; stroke-width: 1.5;
  stroke-linecap: round; stroke-linejoin: round;
}}
.tree-name {{
  flex: 1;
  overflow: hidden;
  text-overflow: ellipsis;
}}
.tree-empty {{
  padding: 20px 16px;
  text-align: center;
  font-size: 12px;
  color: var(--fg-secondary);
  opacity: 0.7;
}}
.toc-content {{
  flex: 1;
  overflow-y: auto;
  padding: 4px 8px 16px;
}}
.toc-list {{ list-style: none; padding: 0; margin: 0; }}
.toc-link {{
  display: block;
  padding: 4px 10px;
  font-size: 13px;
  line-height: 1.5;
  color: var(--fg-secondary);
  text-decoration: none;
  border-left: 2px solid transparent;
  border-radius: 3px;
  transition: background .12s, color .12s, border-color .12s;
  word-break: break-word;
  cursor: pointer;
}}
.toc-link:hover {{ background: var(--accent-light); color: var(--fg); }}
.toc-link.active {{
  color: var(--accent);
  border-left-color: var(--accent);
  background: var(--accent-light);
  font-weight: 600;
}}
.toc-link[data-level="1"] {{ padding-left: 12px; font-weight: 600; }}
.toc-link[data-level="2"] {{ padding-left: 24px; }}
.toc-link[data-level="3"] {{ padding-left: 36px; font-size: 12.5px; }}

.toc-resizer {{
  position: absolute;
  top: 0;
  right: -2px;
  width: 5px;
  height: 100%;
  cursor: ew-resize;
  z-index: 10;
  background: transparent;
  transition: background .15s;
}}
.toc-resizer:hover,
.toc-resizer.dragging {{ background: var(--accent); opacity: 0.4; }}

.toc-toggle {{
  position: absolute;
  top: 14px;
  width: 18px;
  height: 40px;
  border: 1px solid var(--border);
  border-left: none;
  border-radius: 0 6px 6px 0;
  background: var(--block-bg);
  cursor: pointer;
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 0;
  z-index: 15;
  transition: background .12s, left .2s ease, transform .2s ease;
  box-shadow: 1px 1px 3px rgba(0,0,0,.06);
}}
.toc-toggle svg {{
  width: 10px;
  height: 10px;
  stroke: var(--fg-secondary);
  fill: none;
  stroke-width: 2;
  stroke-linecap: round;
  stroke-linejoin: round;
  transition: transform .25s ease;
}}
.toc-toggle:hover {{ background: var(--accent-light); }}
.toc-toggle:hover svg {{ stroke: var(--accent); }}
.toc-toggle.collapsed svg {{ transform: rotate(180deg); }}

/* Editor pane */
.editor-pane {{
  flex: 1 1 50%;
  min-width: 0;
  display: flex;
  flex-direction: column;
  background: var(--editor-bg);
  border: 1px solid var(--border);
  border-radius: var(--radius);
  overflow: hidden;
}}

/* Markdown toolbar */
.md-toolbar {{
  flex-shrink: 0;
  display: flex;
  align-items: center;
  gap: 2px;
  padding: 6px 8px;
  border-bottom: 1px solid var(--border);
  background: var(--titlebar-bg);
  overflow-x: auto;
  overflow-y: hidden;
  scrollbar-width: none;
  -ms-overflow-style: none;
}}
.md-toolbar::-webkit-scrollbar {{ height: 0; display: none; }}
.content-area.mode-edit .md-toolbar {{ padding-right: 200px; }}
.mdb {{
  height: 28px;
  min-width: 28px;
  padding: 0 7px;
  border: 1px solid transparent;
  background: transparent;
  border-radius: 5px;
  cursor: pointer;
  color: var(--fg-secondary);
  font-size: 12.5px;
  font-family: inherit;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  flex-shrink: 0;
  transition: background .12s, color .12s;
}}
.mdb:hover {{ background: var(--btn-hover); color: var(--fg); }}
.mdb:active {{ background: var(--accent-light); }}
.mdb svg {{ width: 15px; height: 15px; stroke: currentColor; fill: none; stroke-width: 1.7; stroke-linecap: round; stroke-linejoin: round; }}
.mdb b, .mdb i {{ font-size: 13px; }}
.mdb-sep {{
  width: 1px;
  height: 18px;
  background: var(--border);
  margin: 0 4px;
  flex-shrink: 0;
}}

/* Slash command popup */
.slash-popup {{
  position: fixed;
  z-index: 9999;
  background: var(--bg);
  border: 1px solid var(--border);
  border-radius: 8px;
  box-shadow: 0 6px 24px rgba(0, 0, 0, 0.18);
  padding: 4px;
  min-width: 220px;
  max-width: 320px;
  max-height: 280px;
  overflow-y: auto;
  font-size: 13px;
}}
.slash-popup[hidden] {{ display: none; }}
.slash-item {{
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 6px 10px;
  border-radius: 5px;
  cursor: pointer;
  color: var(--fg);
  user-select: none;
}}
.slash-item .slash-key {{
  font-size: 11px;
  color: var(--fg-secondary);
  font-family: "Cascadia Code", "Fira Code", Consolas, monospace;
  margin-left: auto;
}}
.slash-item:hover {{ background: var(--btn-hover); }}
.slash-item.selected {{
  background: var(--accent-light);
  color: var(--accent);
}}
.slash-item.selected .slash-key {{ color: var(--accent); }}
.slash-popup::-webkit-scrollbar {{ width: 6px; }}
.slash-popup::-webkit-scrollbar-thumb {{ background: var(--border); border-radius: 3px; }}
.editor-textarea {{
  flex: 1;
  width: 100%;
  border: none;
  outline: none;
  resize: none;
  padding: 16px 18px;
  font-family: "Cascadia Code", "Fira Code", "JetBrains Mono", Consolas, monospace;
  font-size: 13.5px;
  line-height: 1.65;
  background: transparent;
  color: var(--fg);
  tab-size: 2;
}}
.editor-textarea::placeholder {{ color: var(--fg-secondary); opacity: 0.6; }}

/* Splitter between editor and preview */
.split-resizer {{
  width: 6px;
  flex-shrink: 0;
  cursor: ew-resize;
  background: transparent;
  position: relative;
  transition: background .15s;
}}
.split-resizer::before {{
  content: '';
  position: absolute;
  left: 50%;
  top: 50%;
  width: 2px;
  height: 32px;
  background: var(--border);
  border-radius: 2px;
  transform: translate(-50%, -50%);
  transition: background .15s;
}}
.split-resizer:hover::before,
.split-resizer.dragging::before {{ background: var(--accent); }}

/* Main preview scroll */
.main-scroll {{
  flex: 1 1 50%;
  overflow-y: auto;
  overflow-x: hidden;
  min-width: 0;
}}

/* Mode visibility */
.content-area.mode-view .editor-pane,
.content-area.mode-view .split-resizer {{ display: none; }}

.content-area.mode-edit .toc-sidebar,
.content-area.mode-edit .toc-toggle,
.content-area.mode-edit .main-scroll,
.content-area.mode-edit .split-resizer {{ display: none; }}

.content-area.mode-split .toc-sidebar,
.content-area.mode-split .toc-toggle {{ display: none; }}

.content-area.no-doc .toc-sidebar,
.content-area.no-doc .toc-toggle,
.content-area.no-doc .editor-pane,
.content-area.no-doc .split-resizer {{ display: none; }}

/* Drop zone (empty state) */
.drop-zone {{
  flex: 1;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 20px;
  padding: 40px;
}}
.drop-zone.dragging .drop-box {{
  border-color: var(--accent);
  background: var(--drop-bg);
  transform: scale(1.02);
}}
.drop-box {{
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 16px;
  width: 360px;
  height: 260px;
  border: 2px dashed var(--border);
  border-radius: 16px;
  transition: all .2s;
}}
.drop-box svg {{
  width: 56px;
  height: 56px;
  stroke: var(--fg-secondary);
  opacity: .45;
  fill: none;
  stroke-width: 1.5;
  stroke-linecap: round;
  stroke-linejoin: round;
}}
.drop-text {{ font-size: 15px; color: var(--fg-secondary); text-align: center; line-height: 1.6; }}
.drop-text strong {{ display: block; font-size: 17px; color: var(--fg); font-weight: 600; margin-bottom: 4px; }}
.drop-hint {{ font-size: 12px; color: var(--fg-secondary); opacity: .6; }}
.drop-open-btn {{
  margin-top: 8px;
  padding: 8px 18px;
  font-size: 13px;
  font-weight: 500;
  border: 1px solid var(--accent);
  background: transparent;
  color: var(--accent);
  border-radius: 6px;
  cursor: pointer;
  transition: background .12s, color .12s;
}}
.drop-open-btn:hover {{
  background: var(--accent);
  color: #fff;
}}

/* ===== Scrollbar ===== */
.main-scroll::-webkit-scrollbar,
.toc-content::-webkit-scrollbar {{ width: 8px; }}
.main-scroll::-webkit-scrollbar-track,
.toc-content::-webkit-scrollbar-track {{ background: transparent; }}
.main-scroll::-webkit-scrollbar-thumb,
.toc-content::-webkit-scrollbar-thumb {{ background: var(--border); border-radius: 4px; }}
.main-scroll::-webkit-scrollbar-thumb:hover,
.toc-content::-webkit-scrollbar-thumb:hover {{ background: var(--fg-secondary); }}

/* ===== Content typography ===== */
.container {{
  max-width: min(90%, 1920px);
  min-width: 0;
  margin: 0 auto;
  padding: 32px 40px 80px;
}}

/* Block wrappers used for cursor-line highlight in split mode */
.md-block {{
  border-left: 3px solid transparent;
  padding-left: 10px;
  margin-left: -13px;
  transition: border-color .15s ease;
}}
.md-block.cursor-line {{
  border-left-color: var(--accent);
}}
hr.md-block {{
  margin-left: -13px;
  padding-left: 10px;
}}

h1, h2, h3, h4, h5, h6 {{
  font-weight: 700;
  line-height: 1.3;
  margin-top: 2em;
  margin-bottom: 0.6em;
  color: var(--fg);
  letter-spacing: -0.01em;
}}
h1 {{
  font-size: 2.1em;
  margin-top: 0;
  padding-bottom: 0.4em;
  border-bottom: 2px solid var(--accent);
}}
h2 {{
  font-size: 1.55em;
  padding-bottom: 0.3em;
  border-bottom: 1px solid var(--border);
}}
h3 {{ font-size: 1.3em; }}
h4 {{ font-size: 1.1em; }}

p {{ margin-bottom: 1em; }}

a {{
  color: var(--accent);
  text-decoration: none;
  border-bottom: 1px solid transparent;
  transition: border-color 0.2s;
}}
a:hover {{ border-bottom-color: var(--accent); }}

strong {{ font-weight: 650; }}

ul, ol {{ padding-left: 1.8em; margin-bottom: 1em; }}
li {{ margin-bottom: 0.3em; }}
li > ul, li > ol {{ margin-bottom: 0; margin-top: 0.3em; }}
li input[type="checkbox"] {{ margin-right: 0.5em; transform: scale(1.15); accent-color: var(--accent); }}

code {{
  font-family: "Cascadia Code", "Fira Code", "JetBrains Mono", Consolas, monospace;
  font-size: 0.88em;
  background: var(--code-bg);
  padding: 0.15em 0.45em;
  border-radius: 5px;
  border: 1px solid var(--border);
}}

.code-wrapper {{ position: relative; margin-bottom: 1.2em; }}
.code-header {{
  display: flex;
  align-items: center;
  justify-content: space-between;
  background: #2b303b;
  border: 1px solid #3b4048;
  border-bottom: none;
  border-radius: var(--radius) var(--radius) 0 0;
  padding: 4px 8px 4px 14px;
  min-height: 30px;
}}
.code-lang {{
  font-family: "Cascadia Code", "Fira Code", Consolas, monospace;
  font-size: 11px;
  font-weight: 600;
  color: #8b95a7;
  text-transform: uppercase;
  letter-spacing: 0.04em;
}}
.copy-btn {{
  width: 28px; height: 24px;
  border: 1px solid #3b4048;
  border-radius: 5px;
  background: #343945;
  cursor: pointer;
  display: flex;
  align-items: center;
  justify-content: center;
  opacity: 0;
  transition: opacity .15s, background .15s, transform .1s;
}}
.code-wrapper:hover .copy-btn {{ opacity: 1; }}
.copy-btn:hover {{ background: #3e4452; transform: scale(1.05); }}
.copy-btn:active {{ transform: scale(0.95); }}
.copy-btn svg {{ width: 14px; height: 14px; stroke: #8b95a7; fill: none; stroke-width: 1.8; stroke-linecap: round; stroke-linejoin: round; }}
.copy-btn.copied {{ border-color: #22c55e; background: #14291e; opacity: 1; }}
.copy-btn.copied svg {{ stroke: #22c55e; }}

pre {{
  border-radius: var(--radius);
  padding: 0;
  margin: 0;
  overflow-x: auto;
  box-shadow: var(--shadow);
}}
.syntect-block pre {{
  background: #2b303b !important;
  border: 1px solid #3b4048;
  color: #c0c5ce;
}}
.code-header + .syntect-block pre {{
  border-top: none;
  border-radius: 0 0 var(--radius) var(--radius);
}}
pre:not(.syntect-block pre) {{
  background: var(--code-bg);
  border: 1px solid var(--border);
}}
pre code {{
  background: none;
  border: none;
  padding: 0;
  font-size: 0.88em;
  line-height: 1.6;
  display: block;
}}
.code-table {{ width: 100%; border-collapse: collapse; border: none; margin: 0; box-shadow: none; }}
.code-table td {{ border: none; padding: 0; vertical-align: top; }}
.code-table tr:hover td {{ background: transparent; }}
.line-numbers {{
  width: 1px;
  white-space: pre;
  padding: 0.8em 0.8em 0.8em 1em !important;
  text-align: right;
  color: #5c6370;
  font-family: "Cascadia Code", "Fira Code", "JetBrains Mono", Consolas, monospace;
  font-size: 0.88em;
  line-height: 1.6;
  user-select: none;
  border-right: 1px solid #3b4048;
}}
.line-content {{ padding: 0.8em 1.2em !important; overflow-x: auto; }}
.line-content code {{
  font-size: 0.88em;
  line-height: 1.6;
  background: none;
  border: none;
  padding: 0;
  display: block;
  white-space: pre;
}}

blockquote {{
  border-left: 4px solid var(--accent);
  background: var(--block-bg);
  padding: 0.8em 1.2em;
  margin: 0 0 1.2em 0;
  border-radius: 0 var(--radius) var(--radius) 0;
  color: var(--fg-secondary);
}}
blockquote p:last-child {{ margin-bottom: 0; }}

table {{
  width: 100%;
  border-collapse: collapse;
  margin-bottom: 1.2em;
  font-size: 0.94em;
  box-shadow: var(--shadow);
  border-radius: var(--radius);
  overflow: hidden;
}}
thead {{ background: var(--block-bg); }}
th, td {{ padding: 0.7em 1em; text-align: left; border: 1px solid var(--border); }}
th {{
  font-weight: 650;
  font-size: 0.85em;
  text-transform: uppercase;
  letter-spacing: 0.04em;
  color: var(--fg-secondary);
}}
tr:hover td {{ background: var(--block-bg); }}

hr {{
  border: none;
  height: 2px;
  background: linear-gradient(90deg, var(--border), transparent);
  margin: 2.5em 0;
}}

.container img {{
  max-width: 100%;
  height: auto;
  border-radius: var(--radius);
  box-shadow: var(--shadow);
  margin: 0.5em 0;
}}

kbd {{
  font-family: inherit;
  font-size: 0.85em;
  padding: 0.1em 0.5em;
  border: 1px solid var(--border);
  border-radius: 4px;
  box-shadow: 0 1px 0 var(--border);
  background: var(--block-bg);
}}

details {{
  border: 1px solid var(--border);
  border-radius: var(--radius);
  padding: 0.6em 1em;
  margin-bottom: 1em;
}}
summary {{ cursor: pointer; font-weight: 600; }}

.footnote-definition {{ font-size: 0.9em; color: var(--fg-secondary); }}

@media print {{
  .titlebar {{ display: none !important; }}
  body {{ background: #fff; color: #000; }}
  .container {{ max-width: 100%; padding: 20px; }}
  pre {{ box-shadow: none; border: 1px solid #ddd; }}
}}
</style>
</head>
<body>

<div class="titlebar" id="titlebar">
  <div class="titlebar-icon">
    <svg class='titlebar-img' viewBox='0 0 20 20' xmlns='http://www.w3.org/2000/svg'><rect width='20' height='20' rx='4' fill='rgb(58,124,140)'/><path d='M4 14V6l2.5 4L9 6v8' stroke='white' stroke-width='1.6' fill='none' stroke-linecap='round' stroke-linejoin='round'/><path d='M12 10v4m0 0l-1.5-2m1.5 2l1.5-2' stroke='white' stroke-width='1.6' fill='none' stroke-linecap='round' stroke-linejoin='round'/><rect x='11' y='6' width='5' height='3' rx='0.8' fill='none' stroke='white' stroke-width='1' opacity='0.5'/></svg>
    <span class="titlebar-brand">MD Viewer <span class="titlebar-ver">v{ver}</span></span>
  </div>
  <div class="titlebar-controls">
    <button class="titlebar-btn" id="btnMin" title="Minimize">
      <svg viewBox="0 0 10 10"><line x1="1" y1="5" x2="9" y2="5"/></svg>
    </button>
    <button class="titlebar-btn" id="btnMax" title="Maximize">
      <svg viewBox="0 0 10 10"><rect x="1" y="1" width="8" height="8" rx="1"/></svg>
    </button>
    <button class="titlebar-btn close" id="btnClose" title="Close">
      <svg viewBox="0 0 10 10"><line x1="1" y1="1" x2="9" y2="9"/><line x1="9" y1="1" x2="1" y2="9"/></svg>
    </button>
  </div>
</div>

<div class="tab-bar empty" id="tabBar"></div>

<div class="slash-popup" id="slashPopup" hidden></div>

<div class="content-area no-doc" id="contentArea">
  <aside class="toc-sidebar" id="tocSidebar">
    <div class="sidebar-tabs">
      <button class="sidebar-tab active" data-pane="toc">目录</button>
      <button class="sidebar-tab" data-pane="files">文件</button>
    </div>
    <div class="sidebar-pane" id="paneToc">
      <div class="toc-content"><ul class="toc-list" id="tocList"></ul></div>
    </div>
    <div class="sidebar-pane" id="paneFiles" hidden>
      <div class="files-search">
        <input type="text" id="filesSearch" placeholder="搜索 .md 文件..." spellcheck="false">
      </div>
      <div class="files-tree" id="filesTree"></div>
    </div>
    <div class="toc-resizer" id="tocResizer"></div>
  </aside>
  <button class="toc-toggle" id="tocToggle" title="Toggle outline" type="button">
    <svg viewBox="0 0 10 10"><polyline points="6.5,2 3.5,5 6.5,8"/></svg>
  </button>
  <div class="mode-group disabled" id="modeGroup">
    <button class="mode-btn" data-mode="view" title="查看模式">
      <svg viewBox="0 0 24 24"><path d="M1 12s4-8 11-8 11 8 11 8-4 8-11 8S1 12 1 12z"/><circle cx="12" cy="12" r="3"/></svg>
      <span>查看</span>
    </button>
    <button class="mode-btn" data-mode="split" title="查看 + 编辑">
      <svg viewBox="0 0 24 24"><rect x="3" y="4" width="18" height="16" rx="2"/><line x1="12" y1="4" x2="12" y2="20"/></svg>
      <span>双栏</span>
    </button>
    <button class="mode-btn" data-mode="edit" title="编辑模式">
      <svg viewBox="0 0 24 24"><path d="M12 20h9"/><path d="M16.5 3.5a2.121 2.121 0 0 1 3 3L7 19l-4 1 1-4L16.5 3.5z"/></svg>
      <span>编辑</span>
    </button>
  </div>
  <div class="editor-pane" id="editorPane">
    <div class="md-toolbar" id="mdToolbar">
      <button class="mdb" data-action="h1" title="一级标题"><b>H1</b></button>
      <button class="mdb" data-action="h2" title="二级标题"><b>H2</b></button>
      <button class="mdb" data-action="h3" title="三级标题"><b>H3</b></button>
      <span class="mdb-sep"></span>
      <button class="mdb" data-action="bold" title="加粗 (Ctrl+B)"><b style="font-weight:800">B</b></button>
      <button class="mdb" data-action="italic" title="斜体 (Ctrl+I)"><i>I</i></button>
      <button class="mdb" data-action="strike" title="删除线"><span style="text-decoration:line-through">S</span></button>
      <span class="mdb-sep"></span>
      <button class="mdb" data-action="code" title="行内代码">
        <svg viewBox="0 0 24 24"><polyline points="16 18 22 12 16 6"/><polyline points="8 6 2 12 8 18"/></svg>
      </button>
      <button class="mdb" data-action="codeblock" title="代码块">
        <svg viewBox="0 0 24 24"><rect x="3" y="4" width="18" height="16" rx="2"/><polyline points="10 9 7 12 10 15"/><polyline points="14 9 17 12 14 15"/></svg>
      </button>
      <button class="mdb" data-action="quote" title="引用">
        <svg viewBox="0 0 24 24"><path d="M3 21c0-6 4-9 8-9"/><path d="M14 21c0-6 4-9 8-9"/><path d="M3 7v6c0 .5 .5 1 1 1h4c.5 0 1-.5 1-1V8c0-.5-.5-1-1-1H3z"/><path d="M14 7v6c0 .5 .5 1 1 1h4c.5 0 1-.5 1-1V8c0-.5-.5-1-1-1h-5z"/></svg>
      </button>
      <span class="mdb-sep"></span>
      <button class="mdb" data-action="ul" title="无序列表">
        <svg viewBox="0 0 24 24"><line x1="8" y1="6" x2="21" y2="6"/><line x1="8" y1="12" x2="21" y2="12"/><line x1="8" y1="18" x2="21" y2="18"/><circle cx="3.5" cy="6" r="1.2"/><circle cx="3.5" cy="12" r="1.2"/><circle cx="3.5" cy="18" r="1.2"/></svg>
      </button>
      <button class="mdb" data-action="ol" title="有序列表">
        <svg viewBox="0 0 24 24"><line x1="10" y1="6" x2="21" y2="6"/><line x1="10" y1="12" x2="21" y2="12"/><line x1="10" y1="18" x2="21" y2="18"/><path d="M4 4h2v4"/><path d="M4 10h3l-3 4h3"/><path d="M4 16h2.5a1 1 0 010 2H5a1 1 0 000 2h2"/></svg>
      </button>
      <button class="mdb" data-action="task" title="任务列表">
        <svg viewBox="0 0 24 24"><rect x="3" y="3" width="7" height="7" rx="1.2"/><polyline points="4.5 6.5 6 8 8.5 5"/><rect x="3" y="14" width="7" height="7" rx="1.2"/><line x1="13" y1="6.5" x2="21" y2="6.5"/><line x1="13" y1="17.5" x2="21" y2="17.5"/></svg>
      </button>
      <span class="mdb-sep"></span>
      <button class="mdb" data-action="link" title="链接 (Ctrl+K)">
        <svg viewBox="0 0 24 24"><path d="M10 13a5 5 0 007 0l3-3a5 5 0 00-7-7l-1 1"/><path d="M14 11a5 5 0 00-7 0l-3 3a5 5 0 007 7l1-1"/></svg>
      </button>
      <button class="mdb" data-action="image" title="图片">
        <svg viewBox="0 0 24 24"><rect x="3" y="3" width="18" height="18" rx="2"/><circle cx="9" cy="9" r="2"/><polyline points="21 15 16 10 5 21"/></svg>
      </button>
      <button class="mdb" data-action="table" title="表格">
        <svg viewBox="0 0 24 24"><rect x="3" y="3" width="18" height="18" rx="2"/><line x1="3" y1="9" x2="21" y2="9"/><line x1="3" y1="15" x2="21" y2="15"/><line x1="9" y1="3" x2="9" y2="21"/><line x1="15" y1="3" x2="15" y2="21"/></svg>
      </button>
      <button class="mdb" data-action="hr" title="分隔线">
        <svg viewBox="0 0 24 24"><line x1="3" y1="12" x2="21" y2="12"/></svg>
      </button>
      <span class="mdb-sep"></span>
      <button class="mdb" data-action="undo" title="撤销 (Ctrl+Z)">
        <svg viewBox="0 0 24 24"><polyline points="3 7 3 13 9 13"/><path d="M3 13a9 9 0 1 0 3-7"/></svg>
      </button>
      <button class="mdb" data-action="redo" title="重做 (Ctrl+R / Ctrl+Y)">
        <svg viewBox="0 0 24 24"><polyline points="21 7 21 13 15 13"/><path d="M21 13a9 9 0 1 1-3-7"/></svg>
      </button>
      <span class="mdb-sep"></span>
      <button class="mdb" data-action="save" title="保存 (Ctrl+S)">
        <svg viewBox="0 0 24 24"><path d="M19 21H5a2 2 0 01-2-2V5a2 2 0 012-2h11l5 5v11a2 2 0 01-2 2z"/><polyline points="17 21 17 13 7 13 7 21"/><polyline points="7 3 7 8 15 8"/></svg>
      </button>
    </div>
    <textarea class="editor-textarea" id="editorTextarea" spellcheck="false" placeholder="在此编辑 Markdown..."></textarea>
  </div>
  <div class="split-resizer" id="splitResizer"></div>
  <main class="main-scroll" id="mainScroll">
    <div class="container" id="previewContainer"></div>
    <div class="drop-zone" id="dropZone">
      <div class="drop-box">
        <svg viewBox="0 0 64 64">
          <path d="M32 6v36M20 30l12 12 12-12"/>
          <path d="M8 44v10a4 4 0 004 4h40a4 4 0 004-4V44"/>
        </svg>
        <div class="drop-text">
          <strong>拖放 Markdown 文件到此处</strong>
          可同时拖入多个 .md 文件
        </div>
        <button class="drop-open-btn" id="dropOpenBtn">打开文件… (Ctrl+O)</button>
      </div>
      <div class="drop-hint">也可以运行 install.bat 后直接双击 .md 文件打开</div>
    </div>
  </main>
</div>

<script>
(function() {{
  const INITIAL_DOCS = {docs_js};
  const INITIAL_ACTIVE_ID = {active_js};

  const docs = new Map();
  const docOrder = [];
  let activeId = null;
  let previewId = null;
  let lastPermanentId = null;
  let mode = 'view';
  let renderTimer = null;

  const tabBar = document.getElementById('tabBar');
  const modeGroup = document.getElementById('modeGroup');
  const contentArea = document.getElementById('contentArea');
  const editorTA = document.getElementById('editorTextarea');
  const previewContainer = document.getElementById('previewContainer');
  const mainScroll = document.getElementById('mainScroll');
  const dropZone = document.getElementById('dropZone');
  const docBase = document.getElementById('docBase');
  const tocSidebar = document.getElementById('tocSidebar');
  const tocList = document.getElementById('tocList');
  const tocToggle = document.getElementById('tocToggle');
  const tocResizer = document.getElementById('tocResizer');
  const splitResizer = document.getElementById('splitResizer');
  const editorPane = document.getElementById('editorPane');

  function decB64(b64) {{
    if (!b64) return '';
    const bin = atob(b64);
    const bytes = new Uint8Array(bin.length);
    for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
    return new TextDecoder('utf-8').decode(bytes);
  }}
  function encB64(s) {{
    const bytes = new TextEncoder().encode(s);
    let bin = '';
    for (let i = 0; i < bytes.length; i++) bin += String.fromCharCode(bytes[i]);
    return btoa(bin);
  }}

  function setBaseHref(baseDir) {{
    if (!baseDir) {{ docBase.setAttribute('href', ''); return; }}
    const norm = baseDir.replace(/\\/g, '/');
    docBase.setAttribute('href', 'file:///' + norm + '/');
  }}

  function updateWindowTitle(name) {{
    document.title = name ? (name + ' — MD Viewer') : 'MD Viewer';
  }}

  function renderTabBar() {{
    if (docOrder.length === 0) {{
      tabBar.classList.add('empty');
      tabBar.innerHTML = '';
      return;
    }}
    tabBar.classList.remove('empty');
    const frag = document.createDocumentFragment();
    for (const id of docOrder) {{
      const doc = docs.get(id);
      if (!doc) continue;
      const t = document.createElement('div');
      t.className = 'tab' + (id === activeId ? ' active' : '');
      t.dataset.id = String(id);
      t.title = doc.name;
      const label = document.createElement('span');
      label.className = 'tab-label';
      label.textContent = (doc.dirty ? '* ' : '') + doc.name;
      if (doc.dirty) t.classList.add('dirty');
      if (doc.isPreview) {{
        t.classList.add('preview');
        label.style.fontStyle = 'italic';
      }}
      const close = document.createElement('button');
      close.className = 'tab-close';
      close.title = '关闭';
      close.innerHTML = '<svg viewBox="0 0 10 10"><line x1="2" y1="2" x2="8" y2="8"/><line x1="8" y1="2" x2="2" y2="8"/></svg>';
      close.addEventListener('click', (e) => {{ e.stopPropagation(); closeDoc(id); }});
      t.appendChild(label);
      t.appendChild(close);
      t.addEventListener('click', () => switchTo(id));
      t.addEventListener('dblclick', () => promoteToPermanent(id));
      t.addEventListener('mousedown', (e) => {{
        if (e.button === 1) {{ e.preventDefault(); closeDoc(id); }}
      }});
      frag.appendChild(t);
    }}
    tabBar.innerHTML = '';
    tabBar.appendChild(frag);
    // ensure active tab is visible
    const activeEl = tabBar.querySelector('.tab.active');
    if (activeEl) {{
      const r = activeEl.getBoundingClientRect();
      const br = tabBar.getBoundingClientRect();
      if (r.left < br.left) tabBar.scrollLeft -= (br.left - r.left) + 8;
      else if (r.right > br.right) tabBar.scrollLeft += (r.right - br.right) + 8;
    }}
  }}

  function setPreviewHtml(html, preserveScroll) {{
    const prev = preserveScroll ? mainScroll.scrollTop : 0;
    previewContainer.innerHTML = html;
    enhanceCodeBlocks(previewContainer);
    if (mode === 'view') buildTOC();
    if (preserveScroll) mainScroll.scrollTop = prev;
    if (mode === 'split') highlightCursorBlock();
  }}

  // Cached version: take pre-enhanced HTML (already wrapped code blocks) and
  // skip the heavy DOM-mutation pass — just rebind the copy buttons.
  function setPreviewHtmlCached(cached, preserveScroll) {{
    const prev = preserveScroll ? mainScroll.scrollTop : 0;
    previewContainer.innerHTML = cached;
    previewContainer.querySelectorAll('.code-wrapper').forEach(bindCopyButton);
    if (mode === 'view') buildTOC();
    if (preserveScroll) mainScroll.scrollTop = prev;
    if (mode === 'split') highlightCursorBlock();
  }}

  function showEmptyState() {{
    activeId = null;
    contentArea.classList.add('no-doc');
    modeGroup.classList.add('disabled');
    previewContainer.innerHTML = '';
    dropZone.style.display = '';
    editorTA.value = '';
    updateWindowTitle(null);
    renderTabBar();
  }}

  function switchTo(id) {{
    const doc = docs.get(id);
    if (!doc) return;
    activeId = id;
    if (!doc.isPreview) lastPermanentId = id;
    contentArea.classList.remove('no-doc');
    modeGroup.classList.remove('disabled');
    dropZone.style.display = 'none';
    setBaseHref(doc.baseDir);
    updateWindowTitle(doc.name);
    renderTabBar();
    if (editorTA.value !== doc.markdown) editorTA.value = doc.markdown;
    if (doc.enhancedHtml) {{
      setPreviewHtmlCached(doc.enhancedHtml, false);
    }} else {{
      setPreviewHtml(doc.htmlBody, false);
      doc.enhancedHtml = previewContainer.innerHTML;
    }}
    if (mode === 'edit') editorTA.focus();
    if (sidebarPane === 'files') {{
      // Permanent switch may need re-scan (different baseDir);
      // preview only updates the gray highlight on existing tree.
      if (!doc.isPreview) requestFileTree();
      else renderFileTree();
    }}
  }}

  function closeDoc(id) {{
    const doc = docs.get(id);
    if (!doc) return;
    if (doc.dirty) {{
      try {{
        window.ipc.postMessage('confirm-close-tab:' + id + ':' + encB64(doc.markdown));
      }} catch(_) {{}}
      return;
    }}
    forceCloseDoc(id);
  }}

  function forceCloseDoc(id) {{
    const wasActive = (activeId === id);
    if (!docs.has(id)) return;
    docs.delete(id);
    if (previewId === id) previewId = null;
    if (lastPermanentId === id) lastPermanentId = null;
    const idx = docOrder.indexOf(id);
    if (idx >= 0) docOrder.splice(idx, 1);
    try {{ window.ipc.postMessage('close-tab:' + id); }} catch(_) {{}}
    if (docOrder.length === 0) {{
      showEmptyState();
      return;
    }}
    if (wasActive) {{
      const nextId = docOrder[Math.min(idx, docOrder.length - 1)];
      switchTo(nextId);
    }} else {{
      renderTabBar();
      if (sidebarPane === 'files') renderFileTree();
    }}
  }}

  function tryCloseWindow() {{
    const dirty = [];
    for (const id of docOrder) {{
      const d = docs.get(id);
      if (d && d.dirty) dirty.push(d);
    }}
    if (dirty.length === 0) {{
      try {{ window.ipc.postMessage('force-close'); }} catch(_) {{}}
      return;
    }}
    const lines = dirty.map(d => d.id + ' ' + encB64(d.markdown)).join('\n');
    try {{ window.ipc.postMessage('confirm-close-window:' + encB64(lines)); }} catch(_) {{}}
  }}

  function addDocFromB64(id, nameB64, baseB64, mdB64, htmlB64, makeActive) {{
    const name = decB64(nameB64);
    const baseDir = decB64(baseB64);
    const markdown = decB64(mdB64);
    const htmlBody = decB64(htmlB64);
    if (docs.has(id)) {{
      const d = docs.get(id);
      d.name = name; d.baseDir = baseDir; d.markdown = markdown; d.htmlBody = htmlBody;
      d.dirty = false;
      d.isPreview = false;
    }} else {{
      docs.set(id, {{id, name, baseDir, markdown, htmlBody, dirty: false, isPreview: false}});
      docOrder.push(id);
    }}
    if (makeActive) switchTo(id);
    else renderTabBar();
  }}

  function addDocPreview(id, nameB64, baseB64, mdB64, htmlB64) {{
    const name = decB64(nameB64);
    const baseDir = decB64(baseB64);
    const markdown = decB64(mdB64);
    const htmlBody = decB64(htmlB64);
    // If a preview tab exists, drop it first to keep only one preview.
    if (previewId !== null && previewId !== id && docs.has(previewId)) {{
      const old = previewId;
      docs.delete(old);
      const idx = docOrder.indexOf(old);
      if (idx >= 0) docOrder.splice(idx, 1);
      try {{ window.ipc.postMessage('close-tab:' + old); }} catch(_) {{}}
    }}
    docs.set(id, {{id, name, baseDir, markdown, htmlBody, dirty: false, isPreview: true}});
    if (!docOrder.includes(id)) docOrder.push(id);
    previewId = id;
    switchTo(id);
  }}

  function replaceDoc(id, nameB64, baseB64, mdB64, htmlB64) {{
    const doc = docs.get(id);
    if (!doc) return;
    doc.name = decB64(nameB64);
    doc.baseDir = decB64(baseB64);
    doc.markdown = decB64(mdB64);
    doc.htmlBody = decB64(htmlB64);
    doc.enhancedHtml = null;
    doc.dirty = false;
    // Replacement keeps preview-ness; it's still a preview tab unless promoted.
    setBaseHref(doc.baseDir);
    updateWindowTitle(doc.name);
    renderTabBar();
    if (activeId !== id) {{
      switchTo(id);
    }} else {{
      if (editorTA.value !== doc.markdown) editorTA.value = doc.markdown;
      setPreviewHtml(doc.htmlBody, false);
      doc.enhancedHtml = previewContainer.innerHTML;
    }}
    // Preview replacement: don't re-scan, but re-render so the gray highlight
    // follows the new file. Permanent replacement may need re-scan.
    if (sidebarPane === 'files') {{
      if (!doc.isPreview) requestFileTree();
      else renderFileTree();
    }}
  }}

  function promoteToPermanent(id) {{
    const doc = docs.get(id);
    if (!doc || !doc.isPreview) return;
    doc.isPreview = false;
    if (previewId === id) previewId = null;
    renderTabBar();
    if (sidebarPane === 'files') renderFileTree();
  }}

  function markSaved(id) {{
    const doc = docs.get(id);
    if (!doc) return;
    doc.dirty = false;
    renderTabBar();
  }}

  function saveFailed(id) {{
    const doc = docs.get(id);
    if (!doc) return;
    console.warn('Save failed for doc', id, doc && doc.name);
  }}

  function saveActive() {{
    if (activeId === null) return;
    const doc = docs.get(activeId);
    if (!doc) return;
    try {{ window.ipc.postMessage('save:' + activeId + ':' + encB64(doc.markdown)); }} catch(_) {{}}
  }}

  function applyRender(id, htmlB64) {{
    const html = decB64(htmlB64);
    const doc = docs.get(id);
    if (doc) {{
      doc.htmlBody = html;
      doc.enhancedHtml = null;
    }}
    if (activeId === id) {{
      setPreviewHtml(html, true);
      if (doc) doc.enhancedHtml = previewContainer.innerHTML;
    }}
  }}

  function setMode(m) {{
    if (m !== 'view' && m !== 'edit' && m !== 'split') return;
    mode = m;
    contentArea.classList.remove('mode-view', 'mode-edit', 'mode-split');
    contentArea.classList.add('mode-' + m);
    document.querySelectorAll('.mode-btn').forEach(b => {{
      b.classList.toggle('active', b.dataset.mode === m);
    }});
    if (m === 'view' && activeId !== null) buildTOC();
    if (m === 'edit') {{ setTimeout(() => editorTA.focus(), 0); }}
    if (m === 'split') highlightCursorBlock();
    else if (m !== 'split') {{
      const prev = previewContainer.querySelector('.md-block.cursor-line');
      if (prev) prev.classList.remove('cursor-line');
    }}
  }}

  // ===== Code block enhancement =====
  const copySvg = '<svg viewBox="0 0 24 24"><rect x="9" y="9" width="12" height="12" rx="2"/><path d="M5 15H4a2 2 0 01-2-2V4a2 2 0 012-2h9a2 2 0 012 2v1"/></svg>';
  const checkSvg = '<svg viewBox="0 0 24 24"><polyline points="4 12 9 17 20 6"/></svg>';

  function bindCopyButton(wrapper) {{
    const btn = wrapper.querySelector('.copy-btn');
    if (!btn || btn.dataset.bound === '1') return;
    btn.dataset.bound = '1';
    btn.addEventListener('click', () => {{
      const raw = wrapper.dataset.raw || '';
      const ta = document.createElement('textarea');
      ta.value = raw;
      ta.style.position = 'fixed';
      ta.style.left = '-9999px';
      document.body.appendChild(ta);
      ta.select();
      document.execCommand('copy');
      document.body.removeChild(ta);
      btn.classList.add('copied');
      btn.innerHTML = checkSvg;
      setTimeout(() => {{
        btn.classList.remove('copied');
        btn.innerHTML = copySvg;
      }}, 1500);
    }});
  }}

  function enhanceCodeBlocks(root) {{
    root.querySelectorAll('.syntect-block').forEach(block => {{
      if (block.parentElement && block.parentElement.classList.contains('code-wrapper')) {{
        // Already wrapped (e.g., restored from cached innerHTML) — just rebind.
        bindCopyButton(block.parentElement);
        return;
      }}
      const pre = block.querySelector('pre');
      if (!pre) return;
      const langName = block.getAttribute('data-lang') || '';
      const rawText = pre.textContent || '';

      const wrapper = document.createElement('div');
      wrapper.className = 'code-wrapper';
      wrapper.dataset.raw = rawText;
      block.parentNode.insertBefore(wrapper, block);

      const header = document.createElement('div');
      header.className = 'code-header';
      const langSpan = document.createElement('span');
      langSpan.className = 'code-lang';
      langSpan.textContent = langName || 'code';
      header.appendChild(langSpan);

      const btn = document.createElement('button');
      btn.className = 'copy-btn';
      btn.title = 'Copy';
      btn.innerHTML = copySvg;
      header.appendChild(btn);
      wrapper.appendChild(header);

      const lines = rawText.replace(/\n$/, '').split('\n');
      const nums = lines.map((_, i) => i + 1).join('\n');

      const codeEl = pre.querySelector('code') || pre;
      const highlightedHtml = codeEl.innerHTML;

      pre.innerHTML = '';
      pre.style.padding = '0';
      const table = document.createElement('table');
      table.className = 'code-table';
      const tr = document.createElement('tr');
      const tdNum = document.createElement('td');
      tdNum.className = 'line-numbers';
      tdNum.textContent = nums;
      const tdCode = document.createElement('td');
      tdCode.className = 'line-content';
      const newCode = document.createElement('code');
      newCode.innerHTML = highlightedHtml;
      newCode.style.background = 'none';
      newCode.style.border = 'none';
      tdCode.appendChild(newCode);
      tr.appendChild(tdNum);
      tr.appendChild(tdCode);
      table.appendChild(tr);
      pre.appendChild(table);
      wrapper.appendChild(block);

      bindCopyButton(wrapper);
    }});
  }}

  // ===== TOC =====
  let tocActive = null;
  function slugify(text) {{
    return text.toLowerCase().trim()
      .replace(/[^\w一-龥\s-]/g, '')
      .replace(/\s+/g, '-')
      .replace(/-+/g, '-')
      .replace(/^-|-$/g, '') || 'h';
  }}
  function buildTOC() {{
    if (!previewContainer || !tocList) return;
    const headings = Array.from(previewContainer.querySelectorAll('h1, h2, h3'));
    tocList.innerHTML = '';
    // Keep the sidebar visible even when the doc has no headings — the Files
    // pane lives in the same sidebar and must stay reachable. Just leave the
    // TOC list empty.
    if (headings.length === 0) {{
      return;
    }}
    tocSidebar.classList.remove('collapsed');
    tocToggle.style.display = '';
    const used = new Map();
    const entries = [];
    headings.forEach(h => {{
      let id = h.id;
      if (!id) {{
        const base = slugify(h.textContent);
        const n = used.get(base) || 0;
        used.set(base, n + 1);
        id = n === 0 ? base : base + '-' + n;
        h.id = id;
      }}
      entries.push({{ el: h, id: id, text: h.textContent, level: parseInt(h.tagName[1]) }});
    }});
    const frag = document.createDocumentFragment();
    entries.forEach(e => {{
      const li = document.createElement('li');
      const a = document.createElement('a');
      a.className = 'toc-link';
      a.href = '#' + e.id;
      a.textContent = e.text;
      a.setAttribute('data-level', e.level);
      a.setAttribute('data-id', e.id);
      a.addEventListener('click', (ev) => {{
        ev.preventDefault();
        const top = e.el.offsetTop - 8;
        mainScroll.scrollTo({{ top: top, behavior: 'smooth' }});
      }});
      li.appendChild(a);
      frag.appendChild(li);
    }});
    tocList.appendChild(frag);
    syncToggleBtn();
    updateActive();
  }}
  function syncToggleBtn() {{
    const collapsed = tocSidebar.classList.contains('collapsed');
    const left = collapsed ? 0 : tocSidebar.offsetWidth;
    tocToggle.style.left = left + 'px';
    tocToggle.classList.toggle('collapsed', collapsed);
  }}
  function updateActive() {{
    if (mode !== 'view') return;
    const threshold = 80;
    let candidate = null;
    const links = Array.from(tocList.querySelectorAll('.toc-link'));
    if (links.length === 0) return;
    links.forEach(a => {{
      const id = a.getAttribute('data-id');
      const el = document.getElementById(id);
      if (!el) return;
      const rect = el.getBoundingClientRect();
      const scrollRect = mainScroll.getBoundingClientRect();
      const relTop = rect.top - scrollRect.top;
      if (relTop <= threshold) candidate = id;
    }});
    if (!candidate) candidate = links[0].getAttribute('data-id');
    if (candidate !== tocActive) {{
      links.forEach(a => a.classList.toggle('active', a.getAttribute('data-id') === candidate));
      tocActive = candidate;
      const link = tocList.querySelector('.toc-link[data-id="' + candidate.replace(/"/g, '\\"') + '"]');
      if (link) {{
        const lr = link.getBoundingClientRect();
        const cr = link.closest('.toc-content').getBoundingClientRect();
        if (lr.top < cr.top || lr.bottom > cr.bottom) link.scrollIntoView({{ block: 'nearest' }});
      }}
    }}
  }}
  tocToggle.addEventListener('click', () => {{
    tocSidebar.classList.toggle('collapsed');
    syncToggleBtn();
  }});

  // ===== Sidebar tabs + Files pane =====
  let sidebarPane = 'toc';   // 'toc' | 'files'
  const paneToc = document.getElementById('paneToc');
  const paneFiles = document.getElementById('paneFiles');
  const filesTree = document.getElementById('filesTree');
  const filesSearch = document.getElementById('filesSearch');
  let currentTreeBase = '';
  let currentFiles = [];
  let treeFilter = '';

  function setSidebarPane(name) {{
    sidebarPane = name;
    document.querySelectorAll('.sidebar-tab').forEach(b => {{
      b.classList.toggle('active', b.dataset.pane === name);
    }});
    paneToc.hidden = (name !== 'toc');
    paneFiles.hidden = (name !== 'files');
    if (name === 'files') {{
      requestFileTree();
    }}
  }}

  document.querySelectorAll('.sidebar-tab').forEach(b => {{
    b.addEventListener('click', () => setSidebarPane(b.dataset.pane));
  }});

  function requestFileTree() {{
    if (activeId === null) {{
      currentTreeBase = '';
      currentFiles = [];
      renderFileTree();
      return;
    }}
    const doc = docs.get(activeId);
    if (!doc || !doc.baseDir) {{
      currentTreeBase = '';
      currentFiles = [];
      renderFileTree();
      return;
    }}
    if (doc.baseDir === currentTreeBase && currentFiles.length > 0) {{
      renderFileTree();
      return;
    }}
    try {{
      window.ipc.postMessage('list-md-files:' + encB64(doc.baseDir));
    }} catch(_) {{}}
  }}

  function applyFileTree(baseB64, listB64) {{
    const base = decB64(baseB64);
    const list = decB64(listB64);
    currentTreeBase = base;
    currentFiles = list ? list.split('\n').filter(s => s.length > 0) : [];
    renderFileTree();
  }}

  function docRelPathInTree(doc) {{
    if (!doc || !currentTreeBase) return '';
    const full = (doc.baseDir + '/' + doc.name).replace(/\\/g, '/').replace(/\/+/g, '/');
    const baseRaw = currentTreeBase.replace(/\\/g, '/').replace(/\/+/g, '/');
    const base = baseRaw.endsWith('/') ? baseRaw : baseRaw + '/';
    if (full.toLowerCase().startsWith(base.toLowerCase())) {{
      return full.slice(base.length);
    }}
    return '';
  }}

  // Blue highlight: the most recently switched-to PERMANENT doc.
  function permanentHighlightRelPath() {{
    let pid = null;
    if (activeId !== null) {{
      const d = docs.get(activeId);
      if (d && !d.isPreview) pid = activeId;
    }}
    if (pid === null) pid = lastPermanentId;
    if (pid === null) return '';
    return docRelPathInTree(docs.get(pid));
  }}

  // Gray highlight: preview tab's file, but ONLY while the preview tab is the
  // currently active one. Switching back to a permanent tab clears the gray.
  function previewHighlightRelPath() {{
    if (previewId === null) return '';
    if (activeId !== previewId) return '';
    return docRelPathInTree(docs.get(previewId));
  }}

  const chevronSvg = '<svg viewBox="0 0 8 8"><polyline points="2 1 6 4 2 7"/></svg>';
  const folderSvg = '<svg viewBox="0 0 24 24"><path d="M3 6a2 2 0 012-2h4l2 3h8a2 2 0 012 2v9a2 2 0 01-2 2H5a2 2 0 01-2-2z"/></svg>';
  const fileSvg = '<svg viewBox="0 0 24 24"><path d="M14 3H6a2 2 0 00-2 2v14a2 2 0 002 2h12a2 2 0 002-2V9z"/><polyline points="14 3 14 9 20 9"/></svg>';

  function buildTreeFromPaths(paths) {{
    const root = {{ name: '', children: new Map(), isFile: false, fullPath: '' }};
    for (const p of paths) {{
      const parts = p.split('/');
      let cur = root;
      for (let i = 0; i < parts.length; i++) {{
        const name = parts[i];
        const isLeaf = (i === parts.length - 1);
        if (!cur.children.has(name)) {{
          const node = {{
            name,
            children: new Map(),
            isFile: isLeaf,
            fullPath: parts.slice(0, i + 1).join('/'),
          }};
          cur.children.set(name, node);
        }}
        cur = cur.children.get(name);
      }}
    }}
    return root;
  }}

  function nodeMatchesFilter(node, filter) {{
    if (!filter) return true;
    if (node.isFile) return node.name.toLowerCase().includes(filter);
    for (const c of node.children.values()) {{
      if (nodeMatchesFilter(c, filter)) return true;
    }}
    return false;
  }}

  // expanded folder paths
  const expandedFolders = new Set();
  // Always expand root by default
  expandedFolders.add('');

  function renderFileTree() {{
    if (!filesTree) return;
    filesTree.innerHTML = '';
    if (currentFiles.length === 0) {{
      const empty = document.createElement('div');
      empty.className = 'tree-empty';
      empty.textContent = activeId === null ? '请先打开一个 .md 文件' : '没有找到 .md 文件';
      filesTree.appendChild(empty);
      return;
    }}
    const tree = buildTreeFromPaths(currentFiles);
    const filter = treeFilter.toLowerCase();
    const activeRel = permanentHighlightRelPath();
    const previewRel = previewHighlightRelPath();
    const frag = document.createDocumentFragment();
    renderNodeChildren(tree, frag, 0, filter, activeRel, previewRel);
    if (frag.childNodes.length === 0) {{
      const empty = document.createElement('div');
      empty.className = 'tree-empty';
      empty.textContent = '没有匹配的文件';
      filesTree.appendChild(empty);
    }} else {{
      filesTree.appendChild(frag);
    }}
  }}

  function renderNodeChildren(node, container, depth, filter, activeRel, previewRel) {{
    // Sort: folders first, then files; both alphabetic
    const arr = Array.from(node.children.values()).sort((a, b) => {{
      if (a.isFile !== b.isFile) return a.isFile ? 1 : -1;
      return a.name.localeCompare(b.name, 'zh');
    }});
    for (const child of arr) {{
      if (!nodeMatchesFilter(child, filter)) continue;
      const el = document.createElement('div');
      const padLeft = 6 + depth * 12;
      if (child.isFile) {{
        el.className = 'tree-file';
        const item = document.createElement('div');
        item.className = 'tree-item';
        item.style.paddingLeft = padLeft + 'px';
        if (child.fullPath === activeRel) item.classList.add('active');
        else if (child.fullPath === previewRel) item.classList.add('preview-active');
        item.innerHTML =
          '<span class="chevron"></span>' +
          '<span class="tree-icon">' + fileSvg + '</span>' +
          '<span class="tree-name"></span>';
        item.querySelector('.tree-name').textContent = child.name;
        item.title = child.fullPath;
        let treeClickTimer = null;
        item.addEventListener('click', () => {{
          if (treeClickTimer) return; // ignore second click within window — handled by dblclick
          treeClickTimer = setTimeout(() => {{
            treeClickTimer = null;
            openFromTree(child.fullPath, false);
          }}, 220);
        }});
        item.addEventListener('dblclick', () => {{
          if (treeClickTimer) {{
            clearTimeout(treeClickTimer);
            treeClickTimer = null;
          }}
          openFromTree(child.fullPath, true);
        }});
        el.appendChild(item);
      }} else {{
        el.className = 'tree-folder';
        const isExpanded = filter ? true : expandedFolders.has(child.fullPath);
        if (!isExpanded) el.classList.add('collapsed');
        const item = document.createElement('div');
        item.className = 'tree-item';
        item.style.paddingLeft = padLeft + 'px';
        item.innerHTML =
          '<span class="chevron">' + chevronSvg + '</span>' +
          '<span class="tree-icon">' + folderSvg + '</span>' +
          '<span class="tree-name"></span>';
        item.querySelector('.tree-name').textContent = child.name;
        item.addEventListener('click', () => {{
          if (expandedFolders.has(child.fullPath)) expandedFolders.delete(child.fullPath);
          else expandedFolders.add(child.fullPath);
          renderFileTree();
        }});
        el.appendChild(item);
        const sub = document.createElement('div');
        sub.className = 'tree-children';
        renderNodeChildren(child, sub, depth + 1, filter, activeRel, previewRel);
        el.appendChild(sub);
      }}
      container.appendChild(el);
    }}
  }}

  function openFromTree(relPath, permanent) {{
    if (!currentTreeBase) return;
    const sep = currentTreeBase.endsWith('/') || currentTreeBase.endsWith('\\') ? '' : '/';
    const abs = currentTreeBase + sep + relPath;
    // If file already open, just switch + optionally promote.
    const existing = findDocByAbsPath(abs);
    if (existing !== null) {{
      if (permanent) promoteToPermanent(existing);
      switchTo(existing);
      return;
    }}
    if (permanent) {{
      try {{ window.ipc.postMessage('open-path:' + encB64(abs)); }} catch(_) {{}}
    }} else if (previewId !== null && docs.has(previewId)) {{
      try {{ window.ipc.postMessage('replace-doc:' + previewId + ':' + encB64(abs)); }} catch(_) {{}}
    }} else {{
      try {{ window.ipc.postMessage('open-path-preview:' + encB64(abs)); }} catch(_) {{}}
    }}
  }}

  function findDocByAbsPath(abs) {{
    const target = abs.replace(/\\/g, '/').toLowerCase().replace(/\/+/g, '/');
    for (const id of docOrder) {{
      const d = docs.get(id);
      if (!d) continue;
      const full = (d.baseDir + '/' + d.name).replace(/\\/g, '/').toLowerCase().replace(/\/+/g, '/');
      if (full === target) return id;
    }}
    return null;
  }}

  if (filesSearch) {{
    filesSearch.addEventListener('input', () => {{
      treeFilter = filesSearch.value || '';
      renderFileTree();
    }});
  }}

  // TOC resizer
  (function() {{
    const MIN_W = 180, MAX_W = 480;
    try {{
      const saved = parseInt(localStorage.getItem('mdv-toc-width') || '0', 10);
      if (saved >= MIN_W && saved <= MAX_W) tocSidebar.style.width = saved + 'px';
    }} catch(_) {{}}
    let dragging = false, startX = 0, startW = 0;
    tocResizer.addEventListener('mousedown', (ev) => {{
      dragging = true;
      startX = ev.clientX;
      startW = tocSidebar.offsetWidth;
      tocResizer.classList.add('dragging');
      document.body.style.userSelect = 'none';
      document.body.style.cursor = 'ew-resize';
      ev.preventDefault();
      ev.stopPropagation();
    }});
    document.addEventListener('mousemove', (ev) => {{
      if (!dragging) return;
      let w = startW + (ev.clientX - startX);
      if (w < MIN_W) w = MIN_W;
      if (w > MAX_W) w = MAX_W;
      tocSidebar.style.width = w + 'px';
      tocToggle.style.left = w + 'px';
    }});
    document.addEventListener('mouseup', () => {{
      if (!dragging) return;
      dragging = false;
      tocResizer.classList.remove('dragging');
      document.body.style.userSelect = '';
      document.body.style.cursor = '';
      try {{ localStorage.setItem('mdv-toc-width', tocSidebar.offsetWidth); }} catch(_) {{}}
    }});
  }})();

  // Split resizer (between editor and preview)
  (function() {{
    let dragging = false, startX = 0, startEditor = 0, startPreview = 0;
    // Restore saved split ratio
    try {{
      const r = parseFloat(localStorage.getItem('mdv-split-ratio') || '0.5');
      if (r > 0.15 && r < 0.85) applyRatio(r);
    }} catch(_) {{}}
    function applyRatio(r) {{
      editorPane.style.flex = r.toFixed(4) + ' 1 0';
      mainScroll.style.flex = (1 - r).toFixed(4) + ' 1 0';
    }}
    splitResizer.addEventListener('mousedown', (ev) => {{
      if (mode !== 'split') return;
      dragging = true;
      startX = ev.clientX;
      startEditor = editorPane.offsetWidth;
      startPreview = mainScroll.offsetWidth;
      splitResizer.classList.add('dragging');
      document.body.style.userSelect = 'none';
      document.body.style.cursor = 'ew-resize';
      ev.preventDefault();
      ev.stopPropagation();
    }});
    document.addEventListener('mousemove', (ev) => {{
      if (!dragging) return;
      const dx = ev.clientX - startX;
      const total = startEditor + startPreview;
      let ew = startEditor + dx;
      const min = total * 0.15;
      const max = total * 0.85;
      if (ew < min) ew = min;
      if (ew > max) ew = max;
      const ratio = ew / total;
      applyRatio(ratio);
    }});
    document.addEventListener('mouseup', () => {{
      if (!dragging) return;
      dragging = false;
      splitResizer.classList.remove('dragging');
      document.body.style.userSelect = '';
      document.body.style.cursor = '';
      const total = editorPane.offsetWidth + mainScroll.offsetWidth;
      const ratio = total > 0 ? editorPane.offsetWidth / total : 0.5;
      try {{ localStorage.setItem('mdv-split-ratio', ratio.toFixed(4)); }} catch(_) {{}}
    }});
  }})();

  // Editor input
  function onEditorInput() {{
    if (activeId === null) return;
    const doc = docs.get(activeId);
    if (!doc) return;
    if (doc.markdown !== editorTA.value) {{
      doc.markdown = editorTA.value;
      let needTabRefresh = false;
      if (!doc.dirty) {{
        doc.dirty = true;
        needTabRefresh = true;
      }}
      let promoted = false;
      if (doc.isPreview) {{
        doc.isPreview = false;
        if (previewId === activeId) previewId = null;
        needTabRefresh = true;
        promoted = true;
      }}
      if (needTabRefresh) renderTabBar();
      if (promoted && sidebarPane === 'files') renderFileTree();
    }}
    if (mode === 'split') {{
      highlightCursorBlock();
      if (renderTimer) clearTimeout(renderTimer);
      renderTimer = setTimeout(() => {{
        try {{ window.ipc.postMessage('render:' + activeId + ':' + encB64(doc.markdown)); }} catch(_) {{}}
      }}, 350);
    }}
  }}
  editorTA.addEventListener('input', onEditorInput);
  editorTA.addEventListener('keydown', (e) => {{
    if (e.key === 'Tab') {{
      e.preventDefault();
      editorTA.focus();
      document.execCommand('insertText', false, '  ');
      return;
    }}
  }});

  // ===== Paste image (Ctrl+V on an image in clipboard) =====
  const pendingPastes = [];
  editorTA.addEventListener('paste', (e) => {{
    if (!e.clipboardData) return;
    const items = e.clipboardData.items || [];
    for (let i = 0; i < items.length; i++) {{
      const it = items[i];
      if (it.type && it.type.indexOf('image/') === 0) {{
        const file = it.getAsFile();
        if (!file) continue;
        e.preventDefault();
        if (activeId === null) return;
        const tag = 'paste-' + Date.now() + '-' + Math.floor(Math.random() * 100000);
        const placeholder = '![上传中...](' + tag + ')';
        pendingPastes.push(tag);
        editorTA.focus();
        document.execCommand('insertText', false, placeholder);
        const reader = new FileReader();
        reader.onload = () => {{
          const dataUrl = String(reader.result || '');
          const idx = dataUrl.indexOf(',');
          if (idx < 0) return;
          const b64 = dataUrl.slice(idx + 1);
          try {{ window.ipc.postMessage('paste-image:' + activeId + ':' + b64); }} catch(_) {{}}
        }};
        reader.readAsDataURL(file);
        return;
      }}
    }}
  }});

  function pasteImageInserted(relPathB64) {{
    const relPath = decB64(relPathB64);
    const tag = pendingPastes.shift();
    if (!tag) return;
    const placeholder = '![上传中...](' + tag + ')';
    const replacement = '![](' + relPath + ')';
    const v = editorTA.value;
    const idx = v.indexOf(placeholder);
    if (idx < 0) return;
    editorTA.focus();
    editorTA.selectionStart = idx;
    editorTA.selectionEnd = idx + placeholder.length;
    document.execCommand('insertText', false, replacement);
    const newPos = idx + replacement.length;
    editorTA.selectionStart = editorTA.selectionEnd = newPos;
  }}

  // ===== Slash command popup =====
  const slashPopup = document.getElementById('slashPopup');
  const SLASH_ITEMS = [
    {{ label: 'H1 一级标题',  keys: 'h1 标题 heading',           hint: 'Ctrl+1', action: 'h1' }},
    {{ label: 'H2 二级标题',  keys: 'h2 标题 heading',           hint: 'Ctrl+2', action: 'h2' }},
    {{ label: 'H3 三级标题',  keys: 'h3 标题 heading',           hint: 'Ctrl+3', action: 'h3' }},
    {{ label: '加粗',         keys: 'b bold 加粗',               hint: 'Ctrl+B', action: 'bold' }},
    {{ label: '斜体',         keys: 'i italic 斜体',             hint: 'Ctrl+I', action: 'italic' }},
    {{ label: '删除线',       keys: 'strike 删除 删除线',        hint: 'Ctrl+Shift+X', action: 'strike' }},
    {{ label: '行内代码',     keys: 'code 代码 inline',          hint: 'Ctrl+E', action: 'code' }},
    {{ label: '代码块',       keys: 'codeblock 代码块 fence',    hint: 'Ctrl+Shift+E', action: 'codeblock' }},
    {{ label: '引用',         keys: 'quote 引用',                hint: 'Ctrl+Q', action: 'quote' }},
    {{ label: '无序列表',     keys: 'ul list 列表 无序',         hint: 'Ctrl+L', action: 'ul' }},
    {{ label: '有序列表',     keys: 'ol list 列表 有序',         hint: 'Ctrl+Shift+L', action: 'ol' }},
    {{ label: '任务列表',     keys: 'task todo 任务 列表 复选',  hint: 'Ctrl+T', action: 'task' }},
    {{ label: '链接',         keys: 'link 链接',                 hint: 'Ctrl+K', action: 'link' }},
    {{ label: '图片',         keys: 'image img 图片',            hint: 'Ctrl+Shift+I', action: 'image' }},
    {{ label: '表格',         keys: 'table 表格 grid',           hint: 'Ctrl+Shift+M', action: 'table' }},
    {{ label: '分隔线',       keys: 'hr rule 分隔 分割 横线',    hint: 'Ctrl+Shift+H', action: 'hr' }},
  ];
  let slashActive = false;
  let slashStart = -1;
  let slashQuery = '';
  let slashSelected = 0;
  let slashFiltered = SLASH_ITEMS;

  function getCaretCoords(ta, pos) {{
    const mirror = document.createElement('div');
    const cs = window.getComputedStyle(ta);
    const props = [
      'boxSizing', 'width', 'height', 'overflowX', 'overflowY',
      'borderTopWidth', 'borderRightWidth', 'borderBottomWidth', 'borderLeftWidth',
      'borderStyle',
      'paddingTop', 'paddingRight', 'paddingBottom', 'paddingLeft',
      'fontStyle', 'fontVariant', 'fontWeight', 'fontStretch', 'fontSize',
      'fontSizeAdjust', 'lineHeight', 'fontFamily',
      'textAlign', 'textTransform', 'textIndent', 'textDecoration',
      'letterSpacing', 'wordSpacing', 'tabSize', 'whiteSpace'
    ];
    for (const p of props) {{ mirror.style[p] = cs[p]; }}
    mirror.style.position = 'absolute';
    mirror.style.visibility = 'hidden';
    mirror.style.whiteSpace = 'pre-wrap';
    mirror.style.wordWrap = 'break-word';
    mirror.style.top = '0';
    mirror.style.left = '-9999px';
    document.body.appendChild(mirror);
    mirror.textContent = ta.value.slice(0, pos);
    const span = document.createElement('span');
    span.textContent = ta.value.slice(pos) || '.';
    mirror.appendChild(span);
    const taRect = ta.getBoundingClientRect();
    const spanRect = span.getBoundingClientRect();
    const mirrorRect = mirror.getBoundingClientRect();
    const x = taRect.left + (spanRect.left - mirrorRect.left) - ta.scrollLeft;
    const y = taRect.top + (spanRect.top - mirrorRect.top) - ta.scrollTop;
    document.body.removeChild(mirror);
    const lineHeight = parseFloat(cs.lineHeight) || (parseFloat(cs.fontSize) * 1.2);
    return {{ x, y, lineHeight }};
  }}

  function slashHide() {{
    slashActive = false;
    slashStart = -1;
    slashQuery = '';
    slashSelected = 0;
    slashPopup.hidden = true;
  }}

  function slashRender() {{
    slashPopup.innerHTML = '';
    if (slashFiltered.length === 0) {{
      slashHide();
      return;
    }}
    if (slashSelected >= slashFiltered.length) slashSelected = 0;
    let selectedEl = null;
    slashFiltered.forEach((item, i) => {{
      const row = document.createElement('div');
      row.className = 'slash-item' + (i === slashSelected ? ' selected' : '');
      const label = document.createElement('span');
      label.textContent = item.label;
      row.appendChild(label);
      const key = document.createElement('span');
      key.className = 'slash-key';
      key.textContent = item.hint;
      row.appendChild(key);
      row.addEventListener('mousedown', (e) => {{
        e.preventDefault();
        slashSelected = i;
        slashCommit();
      }});
      slashPopup.appendChild(row);
      if (i === slashSelected) selectedEl = row;
    }});
    if (selectedEl) {{
      selectedEl.scrollIntoView({{ block: 'nearest', inline: 'nearest' }});
    }}
  }}

  function slashPosition() {{
    if (slashStart < 0) return;
    const c = getCaretCoords(editorTA, slashStart);
    slashPopup.style.left = Math.max(8, c.x) + 'px';
    slashPopup.style.top = (c.y + c.lineHeight + 4) + 'px';
  }}

  function slashOpen(pos) {{
    slashActive = true;
    slashStart = pos;
    slashQuery = '';
    slashSelected = 0;
    slashFiltered = SLASH_ITEMS.slice();
    slashPopup.hidden = false;
    slashRender();
    slashPosition();
  }}

  function slashUpdateFilter() {{
    const q = slashQuery.toLowerCase().trim();
    if (!q) slashFiltered = SLASH_ITEMS.slice();
    else slashFiltered = SLASH_ITEMS.filter(it =>
      it.label.toLowerCase().includes(q) || it.keys.toLowerCase().includes(q)
    );
    slashSelected = 0;
    slashRender();
  }}

  function slashCommit() {{
    if (!slashActive) return;
    const item = slashFiltered[slashSelected];
    if (!item) {{ slashHide(); return; }}
    // Remove the "/<query>" from editor
    const v = editorTA.value;
    const endPos = editorTA.selectionStart;
    if (slashStart >= 0 && slashStart < endPos) {{
      editorTA.focus();
      editorTA.selectionStart = slashStart;
      editorTA.selectionEnd = endPos;
      document.execCommand('insertText', false, '');
    }}
    slashHide();
    runMdAction(item.action);
  }}

  function isSlashTrigger(text, pos) {{
    // "/" at the very start, or after whitespace/newline
    if (pos <= 0) return true;
    const prev = text.charAt(pos - 1);
    return prev === '\n' || prev === ' ' || prev === '\t';
  }}

  editorTA.addEventListener('input', () => {{
    const v = editorTA.value;
    const pos = editorTA.selectionStart;
    if (!slashActive) {{
      // detect newly typed "/"
      if (pos > 0 && v.charAt(pos - 1) === '/' && isSlashTrigger(v, pos - 1)) {{
        slashOpen(pos - 1);
      }}
      return;
    }}
    // Already active: update query / cancel
    if (pos < slashStart || pos > slashStart + 80) {{
      slashHide();
      return;
    }}
    if (v.charAt(slashStart) !== '/') {{
      slashHide();
      return;
    }}
    slashQuery = v.slice(slashStart + 1, pos);
    if (/\s/.test(slashQuery)) {{
      slashHide();
      return;
    }}
    slashUpdateFilter();
  }});

  editorTA.addEventListener('keydown', (e) => {{
    if (!slashActive) return;
    if (e.key === 'ArrowDown') {{
      e.preventDefault();
      slashSelected = (slashSelected + 1) % slashFiltered.length;
      slashRender();
    }} else if (e.key === 'ArrowUp') {{
      e.preventDefault();
      slashSelected = (slashSelected - 1 + slashFiltered.length) % slashFiltered.length;
      slashRender();
    }} else if (e.key === 'Enter' || e.key === 'Tab') {{
      e.preventDefault();
      slashCommit();
    }} else if (e.key === 'Escape') {{
      e.preventDefault();
      slashHide();
    }}
  }}, true);

  editorTA.addEventListener('blur', () => {{
    // Hide popup when editor loses focus, but with delay to allow click on popup item.
    setTimeout(() => {{ if (slashActive) slashHide(); }}, 150);
  }});

  // ===== Markdown toolbar actions =====
  // We use execCommand('insertText') so changes are recorded in the textarea's
  // native undo stack (Ctrl+Z works) and the 'input' event fires automatically.
  function insertTextAt(text) {{
    editorTA.focus();
    document.execCommand('insertText', false, text);
  }}
  function wrapSelection(prefix, suffix, placeholder) {{
    if (activeId === null) return;
    placeholder = placeholder || '';
    editorTA.focus();
    const start = editorTA.selectionStart;
    const end = editorTA.selectionEnd;
    const sel = editorTA.value.slice(start, end);
    const content = sel || placeholder;
    insertTextAt(prefix + content + suffix);
    if (!sel && placeholder) {{
      editorTA.selectionStart = start + prefix.length;
      editorTA.selectionEnd = start + prefix.length + placeholder.length;
    }} else {{
      editorTA.selectionStart = start + prefix.length;
      editorTA.selectionEnd = start + prefix.length + content.length;
    }}
  }}
  function prefixLines(prefixFn) {{
    if (activeId === null) return;
    editorTA.focus();
    const start = editorTA.selectionStart;
    const end = editorTA.selectionEnd;
    const v = editorTA.value;
    const lineStart = v.lastIndexOf('\n', start - 1) + 1;
    let lineEnd = v.indexOf('\n', end);
    if (lineEnd === -1) lineEnd = v.length;
    const block = v.slice(lineStart, lineEnd);
    const lines = block.split('\n');
    const out = lines.map((l, i) => prefixFn(i) + l).join('\n');
    editorTA.selectionStart = lineStart;
    editorTA.selectionEnd = lineEnd;
    insertTextAt(out);
    editorTA.selectionStart = lineStart;
    editorTA.selectionEnd = lineStart + out.length;
  }}
  function insertAtCursor(text, cursorOffset, selLen) {{
    if (activeId === null) return;
    editorTA.focus();
    const start = editorTA.selectionStart;
    insertTextAt(text);
    if (typeof cursorOffset === 'number') {{
      editorTA.selectionStart = start + cursorOffset;
      editorTA.selectionEnd = start + cursorOffset + (selLen || 0);
    }}
  }}

  function doUndo() {{
    if (activeId === null) return;
    if (mode === 'view') setMode('edit');
    editorTA.focus();
    document.execCommand('undo');
  }}
  function doRedo() {{
    if (activeId === null) return;
    if (mode === 'view') setMode('edit');
    editorTA.focus();
    document.execCommand('redo');
  }}

  const mdActions = {{
    h1: () => prefixLines(() => '# '),
    h2: () => prefixLines(() => '## '),
    h3: () => prefixLines(() => '### '),
    bold: () => wrapSelection('**', '**', '加粗文本'),
    italic: () => wrapSelection('*', '*', '斜体文本'),
    strike: () => wrapSelection('~~', '~~', '删除线文本'),
    code: () => wrapSelection('`', '`', 'code'),
    codeblock: () => {{
      if (activeId === null) return;
      editorTA.focus();
      const start = editorTA.selectionStart, end = editorTA.selectionEnd;
      const sel = editorTA.value.slice(start, end);
      const content = sel || 'code';
      insertTextAt('\n```\n' + content + '\n```\n');
      const codeStart = start + 5;
      editorTA.selectionStart = codeStart;
      editorTA.selectionEnd = codeStart + content.length;
    }},
    quote: () => prefixLines(() => '> '),
    ul: () => prefixLines(() => '- '),
    ol: () => prefixLines((i) => (i + 1) + '. '),
    task: () => prefixLines(() => '- [ ] '),
    link: () => {{
      if (activeId === null) return;
      editorTA.focus();
      const start = editorTA.selectionStart, end = editorTA.selectionEnd;
      const sel = editorTA.value.slice(start, end);
      const text = sel || '链接文字';
      insertTextAt('[' + text + '](url)');
      const urlStart = start + 1 + text.length + 2;
      editorTA.selectionStart = urlStart;
      editorTA.selectionEnd = urlStart + 3;
    }},
    image: () => insertAtCursor('![alt](url)', 8, 3),
    table: () => insertAtCursor('\n| 列1 | 列2 | 列3 |\n| --- | --- | --- |\n| 单元格 | 单元格 | 单元格 |\n'),
    hr: () => insertAtCursor('\n\n---\n\n'),
    undo: () => doUndo(),
    redo: () => doRedo(),
    save: () => saveActive(),
  }};

  function runMdAction(action) {{
    const fn = mdActions[action];
    if (fn) fn();
  }}

  document.getElementById('mdToolbar').addEventListener('click', (e) => {{
    const btn = e.target.closest('.mdb');
    if (!btn) return;
    runMdAction(btn.dataset.action);
  }});

  // Mode buttons
  document.querySelectorAll('.mode-btn').forEach(b => {{
    b.addEventListener('click', () => setMode(b.dataset.mode));
  }});

  // Window controls
  document.getElementById('btnMin').addEventListener('click', () => window.ipc.postMessage('minimize'));
  document.getElementById('btnMax').addEventListener('click', () => window.ipc.postMessage('maximize'));
  document.getElementById('btnClose').addEventListener('click', () => tryCloseWindow());

  // Titlebar drag + double-click maximize
  const titlebar = document.getElementById('titlebar');
  let lastClickTime = 0;
  titlebar.addEventListener('mousedown', (e) => {{
    if (e.target.closest('.titlebar-controls')) return;
    if (e.target.closest('.tab-bar')) return;
    if (e.target.closest('.mode-group')) return;
    if (e.button !== 0) return;
    const now = Date.now();
    if (now - lastClickTime < 300) {{
      lastClickTime = 0;
      window.ipc.postMessage('maximize');
    }} else {{
      lastClickTime = now;
      window.ipc.postMessage('drag');
    }}
  }});

  // Edge resize
  (function() {{
    const B = 8;
    let resizeDir = '';
    function getDir(e) {{
      const x = e.clientX, y = e.clientY;
      const w = window.innerWidth, h = window.innerHeight;
      const l = x < B, r = x >= w - B, t = y < B, b = y >= h - B;
      if (t && l) return 'topleft';
      if (t && r) return 'topright';
      if (b && l) return 'bottomleft';
      if (b && r) return 'bottomright';
      if (l) return 'left';
      if (r) return 'right';
      if (t) return 'top';
      if (b) return 'bottom';
      return '';
    }}
    const cursors = {{topleft:'nwse-resize',topright:'nesw-resize',bottomleft:'nesw-resize',bottomright:'nwse-resize',left:'ew-resize',right:'ew-resize',top:'ns-resize',bottom:'ns-resize'}};
    document.addEventListener('mousemove', (e) => {{
      resizeDir = getDir(e);
      document.documentElement.style.cursor = resizeDir ? cursors[resizeDir] : '';
    }}, true);
    document.addEventListener('mousedown', (e) => {{
      if (resizeDir) {{
        e.preventDefault();
        e.stopImmediatePropagation();
        window.ipc.postMessage('resize:' + resizeDir);
      }}
    }}, true);
  }})();

  // Scroll listener for TOC + split-mode scroll sync
  let rafId = 0;
  let scrollLock = false;
  function syncScrollFromEditor() {{
    if (mode !== 'split' || scrollLock) return;
    const eMax = (editorTA.scrollHeight - editorTA.clientHeight);
    if (eMax <= 0) return;
    scrollLock = true;
    const ratio = editorTA.scrollTop / eMax;
    const pMax = (mainScroll.scrollHeight - mainScroll.clientHeight);
    if (pMax > 0) mainScroll.scrollTop = ratio * pMax;
    requestAnimationFrame(() => {{ scrollLock = false; }});
  }}
  function syncScrollFromPreview() {{
    if (mode !== 'split' || scrollLock) return;
    const pMax = (mainScroll.scrollHeight - mainScroll.clientHeight);
    if (pMax <= 0) return;
    scrollLock = true;
    const ratio = mainScroll.scrollTop / pMax;
    const eMax = (editorTA.scrollHeight - editorTA.clientHeight);
    if (eMax > 0) editorTA.scrollTop = ratio * eMax;
    requestAnimationFrame(() => {{ scrollLock = false; }});
  }}
  function computeCursorLine() {{
    if (activeId === null) return -1;
    const v = editorTA.value;
    const pos = editorTA.selectionStart;
    return (v.slice(0, pos).match(/\n/g) || []).length;
  }}
  function highlightCursorBlock() {{
    const prev = previewContainer.querySelector('.md-block.cursor-line');
    if (prev) prev.classList.remove('cursor-line');
    if (mode !== 'split') return;
    const cursorLine = computeCursorLine();
    if (cursorLine < 0) return;
    const blocks = previewContainer.querySelectorAll('[data-md-line]');
    let target = null;
    for (let i = 0; i < blocks.length; i++) {{
      const line = parseInt(blocks[i].getAttribute('data-md-line'), 10);
      if (Number.isFinite(line) && line <= cursorLine) target = blocks[i];
      else break;
    }}
    if (target && target.classList.contains('md-block')) {{
      target.classList.add('cursor-line');
    }}
  }}

  function syncPreviewFromCursor() {{
    if (mode !== 'split') return;
    highlightCursorBlock();
    const v = editorTA.value;
    if (!v) return;
    const pos = editorTA.selectionStart;
    const before = v.slice(0, pos);
    const cursorLine = (before.match(/\n/g) || []).length;
    // Prefer anchor-based scroll: align the highlighted block to top of preview.
    const target = previewContainer.querySelector('.md-block.cursor-line');
    if (target) {{
      const pMax = (mainScroll.scrollHeight - mainScroll.clientHeight);
      if (pMax > 0) {{
        scrollLock = true;
        const containerRect = previewContainer.getBoundingClientRect();
        const targetRect = target.getBoundingClientRect();
        const offset = (targetRect.top - containerRect.top) + previewContainer.offsetTop;
        const desired = Math.max(0, Math.min(pMax, offset - mainScroll.clientHeight * 0.15));
        mainScroll.scrollTop = desired;
        requestAnimationFrame(() => {{ scrollLock = false; }});
        return;
      }}
    }}
    // Fallback: line ratio
    const totalLines = (v.match(/\n/g) || []).length;
    const ratio = totalLines > 0 ? cursorLine / totalLines : 0;
    const pMax = (mainScroll.scrollHeight - mainScroll.clientHeight);
    if (pMax > 0) {{
      scrollLock = true;
      const t = ratio * pMax - mainScroll.clientHeight * 0.25;
      mainScroll.scrollTop = Math.max(0, Math.min(pMax, t));
      requestAnimationFrame(() => {{ scrollLock = false; }});
    }}
  }}
  editorTA.addEventListener('click', syncPreviewFromCursor);
  editorTA.addEventListener('keyup', (e) => {{
    if (e.key === 'ArrowUp' || e.key === 'ArrowDown' || e.key === 'ArrowLeft' || e.key === 'ArrowRight'
        || e.key === 'Home' || e.key === 'End' || e.key === 'PageUp' || e.key === 'PageDown') {{
      syncPreviewFromCursor();
    }}
  }});
  editorTA.addEventListener('scroll', syncScrollFromEditor);
  mainScroll.addEventListener('scroll', () => {{
    syncScrollFromPreview();
    if (rafId) return;
    rafId = requestAnimationFrame(() => {{ rafId = 0; updateActive(); }});
  }});
  window.addEventListener('resize', syncToggleBtn);

  // Empty-state dragover hint
  document.addEventListener('dragover', (e) => {{
    e.preventDefault();
    if (activeId === null) dropZone.classList.add('dragging');
  }});
  document.addEventListener('dragleave', (e) => {{
    if (!e.relatedTarget) dropZone.classList.remove('dragging');
  }});
  document.addEventListener('drop', (e) => {{
    e.preventDefault();
    dropZone.classList.remove('dragging');
  }});

  // Expose API to Rust
  window.mdv = {{
    addDoc: addDocFromB64,
    addDocPreview: addDocPreview,
    replaceDoc: replaceDoc,
    applyRender: applyRender,
    applyFileTree: applyFileTree,
    pasteImageInserted: pasteImageInserted,
    markSaved: markSaved,
    saveFailed: saveFailed,
    switchTo: switchTo,
    closeDoc: closeDoc,
    confirmCloseTab: forceCloseDoc,
    tryCloseWindow: tryCloseWindow,
    setMode: setMode,
  }};

  // Global keyboard shortcuts
  document.addEventListener('keydown', (e) => {{
    // Block F5 reload
    if (e.key === 'F5') {{ e.preventDefault(); e.stopPropagation(); return; }}
    if (!(e.ctrlKey || e.metaKey)) return;
    const k = (e.key || '').toLowerCase();
    const shift = e.shiftKey;
    const alt = e.altKey;
    const stop = () => {{ e.preventDefault(); e.stopPropagation(); }};

    // Save / Open
    if (!shift && !alt && k === 's') {{ stop(); saveActive(); return; }}
    if (!shift && !alt && k === 'o') {{ stop(); try {{ window.ipc.postMessage('open-dialog'); }} catch(_) {{}} return; }}

    // Undo / Redo
    if (!shift && !alt && k === 'z') {{ stop(); doUndo(); return; }}
    if (!alt && (k === 'y' || k === 'r' || (shift && k === 'z'))) {{ stop(); doRedo(); return; }}

    // Headings (Ctrl+1/2/3)
    if (!shift && !alt && k === '1') {{ stop(); runMdAction('h1'); return; }}
    if (!shift && !alt && k === '2') {{ stop(); runMdAction('h2'); return; }}
    if (!shift && !alt && k === '3') {{ stop(); runMdAction('h3'); return; }}

    // Bold / Italic
    if (!shift && !alt && k === 'b') {{ stop(); runMdAction('bold'); return; }}
    if (!shift && !alt && k === 'i') {{ stop(); runMdAction('italic'); return; }}
    if (shift && !alt && k === 'i') {{ stop(); runMdAction('image'); return; }}

    // Strikethrough
    if (shift && !alt && k === 'x') {{ stop(); runMdAction('strike'); return; }}

    // Code / Code block
    if (!shift && !alt && k === 'e') {{ stop(); runMdAction('code'); return; }}
    if (shift && !alt && k === 'e') {{ stop(); runMdAction('codeblock'); return; }}

    // Quote
    if (!shift && !alt && k === 'q') {{ stop(); runMdAction('quote'); return; }}

    // Lists
    if (!shift && !alt && k === 'l') {{ stop(); runMdAction('ul'); return; }}
    if (shift && !alt && k === 'l') {{ stop(); runMdAction('ol'); return; }}
    if (!shift && !alt && k === 't') {{ stop(); runMdAction('task'); return; }}

    // Link
    if (!shift && !alt && k === 'k') {{ stop(); runMdAction('link'); return; }}

    // Table / Horizontal rule
    if (shift && !alt && k === 'm') {{ stop(); runMdAction('table'); return; }}
    if (shift && !alt && k === 'h') {{ stop(); runMdAction('hr'); return; }}
  }}, true);

  const dropOpenBtn = document.getElementById('dropOpenBtn');
  if (dropOpenBtn) {{
    dropOpenBtn.addEventListener('click', () => {{
      try {{ window.ipc.postMessage('open-dialog'); }} catch(_) {{}}
    }});
  }}

  // Bootstrap initial state
  setMode('view');
  if (Array.isArray(INITIAL_DOCS) && INITIAL_DOCS.length > 0) {{
    for (const d of INITIAL_DOCS) {{
      docs.set(d.id, {{
        id: d.id,
        name: decB64(d.name),
        baseDir: decB64(d.baseDir),
        markdown: decB64(d.markdown),
        htmlBody: decB64(d.htmlBody),
        dirty: false,
      }});
      docOrder.push(d.id);
    }}
    const initId = INITIAL_ACTIVE_ID !== null ? INITIAL_ACTIVE_ID : docOrder[0];
    switchTo(initId);
  }} else {{
    showEmptyState();
  }}
}})();
</script>
</body>
</html>"#,
        ver = ver,
        docs_js = docs_js,
        active_js = active_js,
    )
}
