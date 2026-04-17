#![windows_subsystem = "windows"]

use pulldown_cmark::{CodeBlockKind, Event as MdEvent, Tag, TagEnd, Options, Parser};
use syntect::highlighting::ThemeSet;
use syntect::html::highlighted_html_for_string;
use syntect::parsing::SyntaxSet;
use std::cell::RefCell;
use std::env;
use std::fs;
use std::path::PathBuf;
use std::rc::Rc;
use tao::dpi::{LogicalSize, PhysicalPosition};
use tao::event::{Event as WinEvent, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoop};
use tao::window::WindowBuilder;
use wry::WebViewBuilder;

fn main() {
    let args: Vec<String> = env::args().collect();

    let (html_content, file_name) = if args.len() > 1 {
        let file_path = PathBuf::from(&args[1]);
        let markdown = match fs::read_to_string(&file_path) {
            Ok(content) => content,
            Err(e) => {
                show_error(&format!("Failed to read file: {}", e));
                return;
            }
        };
        let name = file_path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "Markdown Viewer".to_string());
        let base_dir = file_path
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        (render_markdown(&markdown, &name, &base_dir), name)
    } else {
        (render_empty_page(), "MD Viewer".to_string())
    };

    let event_loop = EventLoop::new();

    let monitor = event_loop
        .primary_monitor()
        .or_else(|| event_loop.available_monitors().next());

    // Load saved window geometry
    let config_path = get_config_path();
    let (win_width, win_height) = load_window_geometry(&config_path);

    let mut window_builder = WindowBuilder::new()
        .with_title(format!("{} — MD Viewer", file_name))
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

    // Get HWND for window control from IPC
    let hwnd = {
        use tao::platform::windows::WindowExtWindows;
        window.hwnd() as isize
    };

    // Add WS_THICKFRAME to allow edge resizing even without decorations
    unsafe {
        #[link(name = "user32")]
        extern "system" {
            fn GetWindowLongPtrW(hwnd: isize, index: i32) -> isize;
            fn SetWindowLongPtrW(hwnd: isize, index: i32, val: isize) -> isize;
            fn SetWindowPos(hwnd: isize, after: isize, x: i32, y: i32, w: i32, h: i32, flags: u32) -> i32;
        }
        let style = GetWindowLongPtrW(hwnd, -16); // GWL_STYLE
        SetWindowLongPtrW(hwnd, -16, style | 0x00040000); // WS_THICKFRAME
        // Apply the style change
        SetWindowPos(hwnd, 0, 0, 0, 0, 0, 0x0027); // SWP_NOMOVE|SWP_NOSIZE|SWP_NOZORDER|SWP_FRAMECHANGED
    }

    // If HTML too large for with_html (~2MB limit), strip base64 images
    let html_content = if html_content.len() > 1_800_000 {
        strip_large_data_uris(&html_content)
    } else {
        html_content
    };

    let webview: Rc<RefCell<Option<wry::WebView>>> = Rc::new(RefCell::new(None));
    let webview_clone = webview.clone();

    let wv = WebViewBuilder::new()
        .with_html(&html_content)
        .with_ipc_handler(move |msg| {
            let body = msg.body();
            unsafe {
                #[link(name = "user32")]
                extern "system" {
                    fn ShowWindow(hwnd: isize, cmd: i32) -> i32;
                    fn PostMessageW(hwnd: isize, msg: u32, wparam: usize, lparam: isize) -> i32;
                    fn IsZoomed(hwnd: isize) -> i32;
                    fn ReleaseCapture() -> i32;
                    fn SendMessageW(hwnd: isize, msg: u32, wparam: usize, lparam: isize) -> isize;
                }
                match body.as_str() {
                    "minimize" => { ShowWindow(hwnd, 6); }
                    "maximize" => {
                        if IsZoomed(hwnd) != 0 {
                            ShowWindow(hwnd, 9);
                        } else {
                            ShowWindow(hwnd, 3);
                        }
                    }
                    "close" => {
                        save_window_geometry_from_hwnd(hwnd);
                        #[link(name = "kernel32")]
                        extern "system" {
                            fn GetCurrentProcess() -> isize;
                            fn TerminateProcess(handle: isize, code: u32) -> i32;
                        }
                        TerminateProcess(GetCurrentProcess(), 0);
                    }
                    "drag" => {
                        ReleaseCapture();
                        SendMessageW(hwnd, 0x00A1, 2, 0); // WM_NCLBUTTONDOWN, HTCAPTION
                    }
                    _ if body.starts_with("resize:") => {
                        let dir: usize = match &body[7..] {
                            "top"         => 12, // HTTOP
                            "bottom"      => 15, // HTBOTTOM
                            "left"        => 10, // HTLEFT
                            "right"       => 11, // HTRIGHT
                            "topleft"     => 13, // HTTOPLEFT
                            "topright"    => 14, // HTTOPRIGHT
                            "bottomleft"  => 16, // HTBOTTOMLEFT
                            "bottomright" => 17, // HTBOTTOMRIGHT
                            _ => 0,
                        };
                        if dir != 0 {
                            ReleaseCapture();
                            SendMessageW(hwnd, 0x00A1, dir, 0); // WM_NCLBUTTONDOWN
                        }
                    }
                    _ => {}
                }
            }
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
                if let Some(path) = paths.iter().find(|p| {
                    p.extension()
                        .map(|e| e == "md" || e == "markdown")
                        .unwrap_or(false)
                }) {
                    // Read file and render, then load into webview
                    if let Ok(markdown) = fs::read_to_string(path) {
                        let fname = path.file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_default();
                        let bdir = path.parent()
                            .map(|p| p.to_string_lossy().to_string())
                            .unwrap_or_default();
                        let html = render_markdown(&markdown, &fname, &bdir);
                        if let Some(wv) = webview_clone.borrow().as_ref() {
                            let escaped = html.replace('\\', "\\\\")
                                .replace('`', "\\`")
                                .replace("${", "\\${");
                            let _ = wv.evaluate_script(
                                &format!("document.open();document.write(`{}`);document.close();", escaped)
                            );
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
        if let WinEvent::WindowEvent {
            event: WindowEvent::CloseRequested,
            ..
        } = event
        {
            save_window_geometry_from_hwnd(hwnd);
            unsafe {
                #[link(name = "kernel32")]
                extern "system" {
                    fn GetCurrentProcess() -> isize;
                    fn TerminateProcess(handle: isize, code: u32) -> i32;
                }
                TerminateProcess(GetCurrentProcess(), 0);
            }
        }
    });
}

fn show_error(msg: &str) {
    use std::ptr;
    let wide_msg: Vec<u16> = msg.encode_utf16().chain(std::iter::once(0)).collect();
    let wide_title: Vec<u16> = "MD Viewer"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    unsafe {
        #[link(name = "user32")]
        extern "system" {
            fn MessageBoxW(hwnd: *mut u8, text: *const u16, caption: *const u16, typ: u32) -> i32;
        }
        MessageBoxW(
            ptr::null_mut(),
            wide_msg.as_ptr(),
            wide_title.as_ptr(),
            0x10,
        );
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
        if parts.len() != 2 { continue; }
        match parts[0].trim() {
            "width" => w = parts[1].trim().parse().unwrap_or(1100.0),
            "height" => h = parts[1].trim().parse().unwrap_or(800.0),
            _ => {}
        }
    }
    if w < 500.0 { w = 500.0; }
    if h < 400.0 { h = 400.0; }
    if w > 4000.0 { w = 1100.0; }
    if h > 3000.0 { h = 800.0; }
    (w, h)
}

fn save_window_geometry_from_hwnd(hwnd: isize) {
    unsafe {
        #[repr(C)]
        struct Rect { left: i32, top: i32, right: i32, bottom: i32 }
        #[link(name = "user32")]
        extern "system" {
            fn GetWindowRect(hwnd: isize, rect: *mut Rect) -> i32;
            fn IsZoomed(hwnd: isize) -> i32;
        }
        // Don't save if maximized
        if IsZoomed(hwnd) != 0 { return; }
        let mut rc = Rect { left: 0, top: 0, right: 0, bottom: 0 };
        if GetWindowRect(hwnd, &mut rc) != 0 {
            let w = rc.right - rc.left;
            let h = rc.bottom - rc.top;
            let content = format!("width={}\nheight={}\n", w, h);
            let _ = fs::write(get_config_path(), content);
        }
    }
}

fn render_empty_page() -> String {
    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8"><style>
:root {{
  --bg: #ffffff;
  --fg: #1a1a2e;
  --fg-secondary: #555770;
  --border: #e2e4f0;
  --accent: #4361ee;
  --titlebar-bg: #f0f1f8;
  --titlebar-border: #dfe1ed;
  --btn-hover: rgba(0,0,0,.07);
  --btn-close-hover: #e81123;
  --drop-bg: #f8f9fd;
}}
@media (prefers-color-scheme: dark) {{
  :root {{
    --bg: #16161e;
    --fg: #e4e5f1;
    --fg-secondary: #9b9cb8;
    --border: #2a2b3d;
    --accent: #7b93f5;
    --titlebar-bg: #1a1b26;
    --titlebar-border: #24253a;
    --btn-hover: rgba(255,255,255,.08);
    --btn-close-hover: #e81123;
    --drop-bg: #1a1b2a;
  }}
}}
* {{ margin:0; padding:0; box-sizing:border-box; }}
html, body {{ height:100%; overflow:hidden; }}
body {{
  font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, "Noto Sans SC", "Microsoft YaHei", sans-serif;
  background: var(--bg); color: var(--fg);
  display: flex; flex-direction: column;
}}
.titlebar {{
  flex-shrink: 0; height: 38px;
  background: var(--titlebar-bg);
  border-bottom: 1px solid var(--titlebar-border);
  display: flex; align-items: center; justify-content: space-between;
  user-select: none;
}}
.titlebar-icon {{
  display: flex; align-items: center; gap: 8px;
  padding-left: 14px; pointer-events: none;
}}
.titlebar-img {{ width:16px; height:16px; flex-shrink:0; }}
.titlebar-title {{ font-size:12.5px; font-weight:500; color:var(--fg-secondary); }}
.titlebar-ver {{ font-size:10px; opacity:0.5; font-weight:400; }}
.titlebar-controls {{ display:flex; height:100%; }}
.titlebar-btn {{
  width:46px; height:100%; border:none; background:transparent;
  cursor:pointer; display:flex; align-items:center; justify-content:center;
  transition: background .12s;
}}
.titlebar-btn svg {{ width:10px; height:10px; fill:none; stroke:var(--fg-secondary); stroke-width:1.2; stroke-linecap:round; }}
.titlebar-btn:hover {{ background:var(--btn-hover); }}
.titlebar-btn:hover svg {{ stroke:var(--fg); }}
.titlebar-btn.close:hover {{ background:var(--btn-close-hover); }}
.titlebar-btn.close:hover svg {{ stroke:#fff; }}

.drop-zone {{
  flex: 1;
  display: flex; flex-direction: column;
  align-items: center; justify-content: center;
  gap: 20px;
  padding: 40px;
}}
.drop-zone.dragging .drop-box {{
  border-color: var(--accent);
  background: var(--drop-bg);
  transform: scale(1.02);
}}
.drop-box {{
  display: flex; flex-direction: column;
  align-items: center; justify-content: center;
  gap: 16px;
  width: 360px; height: 260px;
  border: 2px dashed var(--border);
  border-radius: 16px;
  transition: all .2s;
}}
.drop-box svg {{
  width: 56px; height: 56px;
  stroke: var(--fg-secondary); opacity: .45;
  fill: none; stroke-width: 1.5; stroke-linecap: round; stroke-linejoin: round;
}}
.drop-text {{
  font-size: 15px; color: var(--fg-secondary);
  text-align: center; line-height: 1.6;
}}
.drop-text strong {{
  display: block; font-size: 17px;
  color: var(--fg); font-weight: 600;
  margin-bottom: 4px;
}}
.drop-hint {{
  font-size: 12px; color: var(--fg-secondary); opacity: .6;
}}
</style>
</head>
<body>
<div class="titlebar">
  <div class="titlebar-icon">
    <svg class='titlebar-img' viewBox='0 0 20 20' xmlns='http://www.w3.org/2000/svg'><rect width='20' height='20' rx='4' fill='rgb(58,124,140)'/><path d='M4 14V6l2.5 4L9 6v8' stroke='white' stroke-width='1.6' fill='none' stroke-linecap='round' stroke-linejoin='round'/><path d='M12 10v4m0 0l-1.5-2m1.5 2l1.5-2' stroke='white' stroke-width='1.6' fill='none' stroke-linecap='round' stroke-linejoin='round'/><rect x='11' y='6' width='5' height='3' rx='0.8' fill='none' stroke='white' stroke-width='1' opacity='0.5'/></svg>
    <span class="titlebar-title">MD Viewer <span class="titlebar-ver">v{ver}</span></span>
  </div>
  <div class="titlebar-controls">
    <button class="titlebar-btn" onclick="window.ipc.postMessage('minimize')">
      <svg viewBox="0 0 10 10"><line x1="1" y1="5" x2="9" y2="5"/></svg>
    </button>
    <button class="titlebar-btn" onclick="window.ipc.postMessage('maximize')">
      <svg viewBox="0 0 10 10"><rect x="1" y="1" width="8" height="8" rx="1"/></svg>
    </button>
    <button class="titlebar-btn close" onclick="window.ipc.postMessage('close')">
      <svg viewBox="0 0 10 10"><line x1="1" y1="1" x2="9" y2="9"/><line x1="9" y1="1" x2="1" y2="9"/></svg>
    </button>
  </div>
</div>

<div class="drop-zone" id="dropZone">
  <div class="drop-box">
    <svg viewBox="0 0 64 64">
      <path d="M32 6v36M20 30l12 12 12-12"/>
      <path d="M8 44v10a4 4 0 004 4h40a4 4 0 004-4V44"/>
    </svg>
    <div class="drop-text">
      <strong>拖放 Markdown 文件到此处</strong>
      将 .md 文件拖入窗口即可预览
    </div>
  </div>
  <div class="drop-hint">也可以运行 install.bat 后直接双击 .md 文件打开</div>
</div>

<script>
// Edge resize (capture phase to override scrollbar)
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

// Titlebar drag + double-click maximize
const titlebar = document.querySelector('.titlebar');
let lastClickTime = 0;
titlebar.addEventListener('mousedown', (e) => {{
  if (e.target.closest('.titlebar-controls')) return;
  const now = Date.now();
  if (now - lastClickTime < 300) {{
    lastClickTime = 0;
    window.ipc.postMessage('maximize');
  }} else {{
    lastClickTime = now;
    window.ipc.postMessage('drag');
  }}
}});

const dz = document.getElementById('dropZone');
document.addEventListener('dragover', (e) => {{ e.preventDefault(); dz.classList.add('dragging'); }});
document.addEventListener('dragleave', (e) => {{ if (!e.relatedTarget) dz.classList.remove('dragging'); }});
document.addEventListener('drop', (e) => {{
  e.preventDefault();
  dz.classList.remove('dragging');
  const file = e.dataTransfer.files[0];
  if (file && file.name.match(/\.m(d|arkdown)$/i)) {{
    window.ipc.postMessage('open:' + file.path);
  }}
}});
</script>
</body>
</html>"#,
        ver = env!("CARGO_PKG_VERSION")
    )
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
    let mime = match path.extension().and_then(|e| e.to_str()).map(|e| e.to_lowercase()).as_deref() {
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
        if chunk.len() > 1 { result.push(CHARS[((n >> 6) & 63) as usize] as char); } else { result.push('='); }
        if chunk.len() > 2 { result.push(CHARS[(n & 63) as usize] as char); } else { result.push('='); }
    }
    result
}

fn strip_large_data_uris(html: &str) -> String {
    // Replace data:image base64 URIs larger than 200KB with a placeholder
    let mut result = String::with_capacity(html.len() / 2);
    let mut pos = 0;
    while pos < html.len() {
        if let Some(idx) = html[pos..].find("data:image") {
            let abs = pos + idx;
            // Find the end of this data URI (next quote)
            if let Some(end) = html[abs..].find('"').or_else(|| html[abs..].find('\'')) {
                let uri = &html[abs..abs + end];
                if uri.len() > 200_000 {
                    result.push_str(&html[pos..abs]);
                    // Skip the large URI, keep empty
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
            None => { result.push_str(&html[pos..]); break; }
        };
        let tag = &html[img_abs..img_abs + tag_end + 1];
        if let Some(replaced_tag) = replace_img_src(tag, base_dir) {
            result.push_str(&html[pos..img_abs]);
            result.push_str(&replaced_tag);
        } else {
            // Keep original tag, continue searching
            result.push_str(&html[pos..img_abs + tag_end + 1]);
        }
        pos = img_abs + tag_end + 1;
    }
    result
}

fn replace_img_src(tag: &str, base_dir: &str) -> Option<String> {
    // Match src="..." or src='...'
    let src_pos = tag.find("src=\"").or_else(|| tag.find("src='"))?;
    let quote = tag.as_bytes()[src_pos + 4] as char;
    let val_start = src_pos + 5;
    let val_end = tag[val_start..].find(quote)? + val_start;
    let src_val = &tag[val_start..val_end];

    // Only process local paths
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

fn render_markdown(markdown: &str, title: &str, base_dir: &str) -> String {
    let mut options = Options::all();
    options.remove(Options::ENABLE_SMART_PUNCTUATION);

    let ss = SyntaxSet::load_defaults_newlines();
    let ts = ThemeSet::load_defaults();
    let theme = &ts.themes["base16-ocean.dark"];

    let parser = Parser::new_ext(markdown, options);
    let mut html_body = String::new();
    let mut in_code_block = false;
    let mut code_lang = String::new();
    let mut code_text = String::new();
    let mut in_image = false;

    for event in parser {
        match event {
            MdEvent::Start(Tag::CodeBlock(kind)) => {
                in_code_block = true;
                code_text.clear();
                code_lang = match kind {
                    CodeBlockKind::Fenced(lang) => lang.to_string(),
                    _ => String::new(),
                };
            }
            MdEvent::End(TagEnd::CodeBlock) => {
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
            }
            MdEvent::Text(text) if in_code_block => {
                code_text.push_str(&text);
            }
            // Intercept images to embed as base64
            MdEvent::Start(Tag::Image { dest_url, title, .. }) => {
                let src = match image_to_data_uri(&dest_url, base_dir) {
                    Some(data_uri) => data_uri,
                    None => dest_url.to_string(),
                };
                html_body.push_str(&format!(
                    "<img src=\"{}\" alt=\"",
                    html_escape(&src)
                ));
                if !title.is_empty() {
                    // title will be added after alt
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
            other => {
                pulldown_cmark::html::push_html(&mut html_body, std::iter::once(other));
            }
        }
    }

    // Post-process: replace local image src in raw HTML <img> tags with base64
    let html_body = embed_local_images(&html_body, base_dir);

    let base_url = format!("file:///{}", base_dir.replace('\\', "/"));

    let escaped_title = title
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");

    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<base href="{base_url}/">
<title>{escaped_title}</title>
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
  --btn-hover: rgba(0,0,0,.07);
  --btn-close-hover: #e81123;
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
    --btn-hover: rgba(255,255,255,.08);
    --btn-close-hover: #e81123;
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
  padding: 0;
  margin: 0;
  height: 100%;
  display: flex;
  flex-direction: column;
  overflow: hidden;
}}

.content-area {{
  flex: 1;
  display: flex;
  min-height: 0;
  margin: 0 8px 8px 8px;
  position: relative;
}}

/* ===== TOC Sidebar ===== */
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
.toc-header {{
  padding: 14px 16px 8px;
  font-size: 11px;
  font-weight: 650;
  text-transform: uppercase;
  letter-spacing: 0.08em;
  color: var(--fg-secondary);
  flex-shrink: 0;
}}
.toc-content {{
  flex: 1;
  overflow-y: auto;
  padding: 4px 8px 16px;
}}
.toc-list {{ list-style: none; padding: 0; margin: 0; }}
.toc-list li {{ margin: 0; }}
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
.toc-link:hover {{
  background: var(--accent-light);
  color: var(--fg);
  border-bottom-color: transparent;
}}
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
.toc-resizer.dragging {{
  background: var(--accent);
  opacity: 0.4;
}}

.main-scroll {{
  flex: 1;
  overflow-y: auto;
  overflow-x: hidden;
  min-width: 0;
}}

/* Toggle button that hugs the right edge of the sidebar */
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
.toc-toggle:hover {{
  background: var(--accent-light);
}}
.toc-toggle:hover svg {{
  stroke: var(--accent);
}}
.toc-toggle.collapsed svg {{
  transform: rotate(180deg);
}}

/* ===== Custom Title Bar ===== */
.titlebar {{
  flex-shrink: 0;
  height: 38px;
  background: var(--titlebar-bg);
  border-bottom: 1px solid var(--titlebar-border);
  display: flex;
  align-items: center;
  justify-content: space-between;
  z-index: 9999;
  user-select: none;
}}

.titlebar-icon {{
  display: flex;
  align-items: center;
  gap: 8px;
  padding-left: 14px;
  pointer-events: none;
}}

.titlebar-img {{
  width: 16px;
  height: 16px;
  flex-shrink: 0;
}}

.titlebar-title {{
  font-size: 12.5px;
  font-weight: 500;
  color: var(--fg-secondary);
  letter-spacing: 0.01em;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}}

.titlebar-ver {{
  font-size: 10px;
  opacity: 0.5;
  font-weight: 400;
}}

.titlebar-controls {{
  display: flex;
  height: 100%;
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

.titlebar-btn:hover {{
  background: var(--btn-hover);
}}

.titlebar-btn:hover svg {{
  stroke: var(--fg);
}}

.titlebar-btn.close:hover {{
  background: var(--btn-close-hover);
}}

.titlebar-btn.close:hover svg {{
  stroke: #fff;
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

/* ===== Content ===== */
.container {{
  max-width: min(90%, 1920px);
  min-width: 480px;
  margin: 0 auto;
  padding: 32px 40px 80px;
  animation: fadeIn 0.25s ease-out;
}}

@keyframes fadeIn {{
  from {{ opacity: 0; transform: translateY(8px); }}
  to {{ opacity: 1; transform: translateY(0); }}
}}

/* Typography */
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

/* Lists */
ul, ol {{
  padding-left: 1.8em;
  margin-bottom: 1em;
}}
li {{ margin-bottom: 0.3em; }}
li > ul, li > ol {{
  margin-bottom: 0;
  margin-top: 0.3em;
}}

li input[type="checkbox"] {{
  margin-right: 0.5em;
  transform: scale(1.15);
  accent-color: var(--accent);
}}

/* Code */
code {{
  font-family: "Cascadia Code", "Fira Code", "JetBrains Mono", Consolas, monospace;
  font-size: 0.88em;
  background: var(--code-bg);
  padding: 0.15em 0.45em;
  border-radius: 5px;
  border: 1px solid var(--border);
}}

/* Code block wrapper */
.code-wrapper {{
  position: relative;
  margin-bottom: 1.2em;
}}

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
  width: 28px;
  height: 24px;
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

.code-wrapper:hover .copy-btn {{
  opacity: 1;
}}

.copy-btn:hover {{
  background: #3e4452;
  transform: scale(1.05);
}}

.copy-btn:active {{
  transform: scale(0.95);
}}

.copy-btn svg {{
  width: 14px;
  height: 14px;
  stroke: #8b95a7;
  fill: none;
  stroke-width: 1.8;
  stroke-linecap: round;
  stroke-linejoin: round;
}}

.copy-btn.copied {{
  border-color: #22c55e;
  background: #14291e;
  opacity: 1;
}}

.copy-btn.copied svg {{
  stroke: #22c55e;
}}

pre {{
  border-radius: var(--radius);
  padding: 0;
  margin: 0;
  overflow-x: auto;
  box-shadow: var(--shadow);
}}

/* Syntect-highlighted pre: always dark */
.syntect-block pre {{
  background: #2b303b !important;
  border: 1px solid #3b4048;
  color: #c0c5ce;
}}

.code-header + .syntect-block pre {{
  border-top: none;
  border-radius: 0 0 var(--radius) var(--radius);
}}

/* Non-highlighted pre (no language) */
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

/* Line numbers table */
.code-table {{
  width: 100%;
  border-collapse: collapse;
  border: none;
  margin: 0;
  box-shadow: none;
}}

.code-table td {{
  border: none;
  padding: 0;
  vertical-align: top;
}}

.code-table tr:hover td {{
  background: transparent;
}}

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

.line-content {{
  padding: 0.8em 1.2em !important;
  overflow-x: auto;
}}

.line-content code {{
  font-size: 0.88em;
  line-height: 1.6;
  background: none;
  border: none;
  padding: 0;
  display: block;
  white-space: pre;
}}

/* Blockquote */
blockquote {{
  border-left: 4px solid var(--accent);
  background: var(--block-bg);
  padding: 0.8em 1.2em;
  margin: 0 0 1.2em 0;
  border-radius: 0 var(--radius) var(--radius) 0;
  color: var(--fg-secondary);
}}
blockquote p:last-child {{ margin-bottom: 0; }}

/* Table */
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
th, td {{
  padding: 0.7em 1em;
  text-align: left;
  border: 1px solid var(--border);
}}
th {{
  font-weight: 650;
  font-size: 0.85em;
  text-transform: uppercase;
  letter-spacing: 0.04em;
  color: var(--fg-secondary);
}}
tr:hover td {{ background: var(--block-bg); }}

/* Horizontal rule */
hr {{
  border: none;
  height: 2px;
  background: linear-gradient(90deg, var(--border), transparent);
  margin: 2.5em 0;
}}

/* Images */
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

.footnote-definition {{
  font-size: 0.9em;
  color: var(--fg-secondary);
}}

@media print {{
  .titlebar {{ display: none !important; }}
  body {{ background: #fff; color: #000; }}
  .container {{ max-width: 100%; padding: 20px; }}
  pre {{ box-shadow: none; border: 1px solid #ddd; }}
}}
</style>
</head>
<body>

<!-- Custom Title Bar -->
<div class="titlebar">
  <div class="titlebar-icon">
    <svg class='titlebar-img' viewBox='0 0 20 20' xmlns='http://www.w3.org/2000/svg'><rect width='20' height='20' rx='4' fill='rgb(58,124,140)'/><path d='M4 14V6l2.5 4L9 6v8' stroke='white' stroke-width='1.6' fill='none' stroke-linecap='round' stroke-linejoin='round'/><path d='M12 10v4m0 0l-1.5-2m1.5 2l1.5-2' stroke='white' stroke-width='1.6' fill='none' stroke-linecap='round' stroke-linejoin='round'/><rect x='11' y='6' width='5' height='3' rx='0.8' fill='none' stroke='white' stroke-width='1' opacity='0.5'/></svg>
    <span class="titlebar-title">{escaped_title} — MD Viewer <span class="titlebar-ver">v{ver}</span></span>
  </div>
  <div class="titlebar-controls">
    <button class="titlebar-btn" onclick="window.ipc.postMessage('minimize')" title="Minimize">
      <svg viewBox="0 0 10 10"><line x1="1" y1="5" x2="9" y2="5"/></svg>
    </button>
    <button class="titlebar-btn" onclick="window.ipc.postMessage('maximize')" title="Maximize">
      <svg viewBox="0 0 10 10"><rect x="1" y="1" width="8" height="8" rx="1"/></svg>
    </button>
    <button class="titlebar-btn close" onclick="window.ipc.postMessage('close')" title="Close">
      <svg viewBox="0 0 10 10"><line x1="1" y1="1" x2="9" y2="9"/><line x1="9" y1="1" x2="1" y2="9"/></svg>
    </button>
  </div>
</div>

<div class="content-area">
<aside class="toc-sidebar" id="tocSidebar">
  <div class="toc-header">目录</div>
  <div class="toc-content"><ul class="toc-list" id="tocList"></ul></div>
  <div class="toc-resizer" id="tocResizer"></div>
</aside>
<button class="toc-toggle" id="tocToggle" title="Toggle outline" type="button">
  <svg viewBox="0 0 10 10"><polyline points="6.5,2 3.5,5 6.5,8"/></svg>
</button>
<main class="main-scroll" id="mainScroll">
<div class="container">
{html_body}
</div>
</main>
</div>

<script>
// Edge resize (capture phase to override scrollbar)
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

// Titlebar drag + double-click maximize
const titlebar = document.querySelector('.titlebar');
let lastClickTime = 0;
titlebar.addEventListener('mousedown', (e) => {{
  if (e.target.closest('.titlebar-controls')) return;
  const now = Date.now();
  if (now - lastClickTime < 300) {{
    // Double click
    lastClickTime = 0;
    window.ipc.postMessage('maximize');
  }} else {{
    lastClickTime = now;
    window.ipc.postMessage('drag');
  }}
}});

// Enhance syntect code blocks with line numbers, header, and copy button
const copySvg = '<svg viewBox="0 0 24 24"><rect x="9" y="9" width="12" height="12" rx="2"/><path d="M5 15H4a2 2 0 01-2-2V4a2 2 0 012-2h9a2 2 0 012 2v1"/></svg>';
const checkSvg = '<svg viewBox="0 0 24 24"><polyline points="4 12 9 17 20 6"/></svg>';

document.querySelectorAll('.syntect-block').forEach(block => {{
  const pre = block.querySelector('pre');
  if (!pre) return;
  const langName = block.getAttribute('data-lang') || '';
  const rawText = pre.textContent || '';

  // Wrapper
  const wrapper = document.createElement('div');
  wrapper.className = 'code-wrapper';
  block.parentNode.insertBefore(wrapper, block);

  // Header bar
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

  // Build line numbers alongside highlighted code
  const lines = rawText.replace(/\n$/, '').split('\n');
  const nums = lines.map((_, i) => i + 1).join('\n');

  // Get the highlighted HTML content from syntect's <code>
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

  // Copy handler
  btn.addEventListener('click', () => {{
    const ta = document.createElement('textarea');
    ta.value = rawText;
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
}});

// ===== TOC (Table of Contents) =====
(function() {{
  const container = document.querySelector('.container');
  const tocList = document.getElementById('tocList');
  const sidebar = document.getElementById('tocSidebar');
  const mainScroll = document.getElementById('mainScroll');
  const toggleBtn = document.getElementById('tocToggle');
  if (!container || !tocList || !sidebar || !mainScroll || !toggleBtn) return;

  const headings = Array.from(container.querySelectorAll('h1, h2, h3'));
  if (headings.length === 0) {{
    sidebar.classList.add('collapsed');
    toggleBtn.style.display = 'none';
    return;
  }}

  function syncToggleBtn() {{
    const collapsed = sidebar.classList.contains('collapsed');
    const left = collapsed ? 0 : sidebar.offsetWidth;
    toggleBtn.style.left = left + 'px';
    toggleBtn.classList.toggle('collapsed', collapsed);
  }}
  toggleBtn.addEventListener('click', () => {{
    sidebar.classList.toggle('collapsed');
    syncToggleBtn();
  }});

  function slugify(text) {{
    return text.toLowerCase().trim()
      .replace(/[^\w\u4e00-\u9fa5\s-]/g, '')
      .replace(/\s+/g, '-')
      .replace(/-+/g, '-')
      .replace(/^-|-$/g, '') || 'h';
  }}

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

  const links = new Map();
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
    links.set(e.id, a);
  }});
  tocList.appendChild(frag);

  // Active highlight: pick the last heading whose top is above a threshold
  let activeId = null;
  function updateActive() {{
    const threshold = 80;
    let candidate = null;
    for (const e of entries) {{
      const rect = e.el.getBoundingClientRect();
      const scrollRect = mainScroll.getBoundingClientRect();
      const relTop = rect.top - scrollRect.top;
      if (relTop <= threshold) candidate = e.id;
      else break;
    }}
    if (!candidate && entries.length > 0) candidate = entries[0].id;
    if (candidate !== activeId) {{
      if (activeId && links.get(activeId)) links.get(activeId).classList.remove('active');
      activeId = candidate;
      if (activeId && links.get(activeId)) {{
        const link = links.get(activeId);
        link.classList.add('active');
        // auto-scroll TOC to keep active visible
        const lr = link.getBoundingClientRect();
        const cr = link.closest('.toc-content').getBoundingClientRect();
        if (lr.top < cr.top || lr.bottom > cr.bottom) {{
          link.scrollIntoView({{ block: 'nearest' }});
        }}
      }}
    }}
  }}
  let rafId = 0;
  mainScroll.addEventListener('scroll', () => {{
    if (rafId) return;
    rafId = requestAnimationFrame(() => {{ rafId = 0; updateActive(); }});
  }});
  updateActive();

  // Resizer with min/max + persistence
  const resizer = document.getElementById('tocResizer');
  const MIN_W = 180, MAX_W = 480;
  try {{
    const saved = parseInt(localStorage.getItem('mdv-toc-width') || '0', 10);
    if (saved >= MIN_W && saved <= MAX_W) sidebar.style.width = saved + 'px';
  }} catch(_) {{}}

  let dragging = false, startX = 0, startW = 0;
  resizer.addEventListener('mousedown', (ev) => {{
    dragging = true;
    startX = ev.clientX;
    startW = sidebar.offsetWidth;
    resizer.classList.add('dragging');
    document.body.style.userSelect = 'none';
    document.body.style.cursor = 'ew-resize';
    toggleBtn.style.transition = 'background .12s';
    ev.preventDefault();
    ev.stopPropagation();
  }});
  document.addEventListener('mousemove', (ev) => {{
    if (!dragging) return;
    let w = startW + (ev.clientX - startX);
    if (w < MIN_W) w = MIN_W;
    if (w > MAX_W) w = MAX_W;
    sidebar.style.width = w + 'px';
    toggleBtn.style.left = w + 'px';
  }});
  document.addEventListener('mouseup', () => {{
    if (!dragging) return;
    dragging = false;
    resizer.classList.remove('dragging');
    document.body.style.userSelect = '';
    document.body.style.cursor = '';
    toggleBtn.style.transition = '';
    try {{ localStorage.setItem('mdv-toc-width', sidebar.offsetWidth); }} catch(_) {{}}
  }});

  syncToggleBtn();
  window.addEventListener('resize', syncToggleBtn);
}})();
</script>
</body>
</html>"#,
        base_url = base_url,
        escaped_title = escaped_title,
        html_body = html_body,
        ver = env!("CARGO_PKG_VERSION"),
    )
}
