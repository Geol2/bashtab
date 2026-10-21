#![windows_subsystem = "windows"]

//! bashtab: a tab strip that hosts Git for Windows' own mintty windows.
//! Each tab is a real mintty process whose window is re-parented into ours.

use std::mem::{size_of, zeroed};
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command};
use std::ptr::{null, null_mut};
use std::time::Instant;

use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Dwm::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Registry::*;
use windows_sys::Win32::UI::HiDpi::*;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

/// Startup script that replaces /etc/profile without spawning processes.
const FAST_RC: &str = include_str!("../fast-rc.sh");

const TIMER_POLL: usize = 1;
const HK_NEW: i32 = 1;
const HK_CLOSE: i32 = 2;
const HK_NEXT: i32 = 3;
const HK_PREV: i32 = 4;
const HK_NEW_WSL: i32 = 5;
const MENU_GIT: usize = 1;
const MENU_WSL: usize = 100; // MENU_WSL + distro index
const HK_TAB: i32 = 10; // HK_TAB + 1..=9
const WM_MOUSELEAVE: u32 = 0x02A3; // not exported by windows-sys

const fn rgb(r: u8, g: u8, b: u8) -> u32 {
    r as u32 | (g as u32) << 8 | (b as u32) << 16
}
const C_BAR: u32 = rgb(32, 32, 32);
const C_ACTIVE: u32 = rgb(12, 12, 12);
const C_HOVER: u32 = rgb(48, 48, 48);
const C_CLOSE_HOVER: u32 = rgb(80, 80, 80);
const C_SEP: u32 = rgb(60, 60, 60);
const C_ACCENT: u32 = rgb(241, 78, 50);
const C_TEXT: u32 = rgb(240, 240, 240);
const C_DIM: u32 = rgb(150, 150, 150);
const C_TERM_BG: u32 = rgb(0, 0, 0);
const C_WSL: u32 = rgb(170, 110, 230);

#[derive(Clone, PartialEq)]
enum Profile {
    GitBash,
    Wsl(String), // distribution name
}

impl Profile {
    fn label(&self) -> &str {
        match self {
            Profile::GitBash => "Git Bash",
            Profile::Wsl(d) => d,
        }
    }
}

struct Tab {
    profile: Profile,
    child: Child,
    hwnd: HWND, // null until mintty's window shows up
    title: String,
    started: Instant,
}

#[derive(Clone, Copy, PartialEq)]
enum Hit {
    None,
    Tab(usize),
    Close(usize),
    Plus,
    Menu,
}

struct App {
    main: HWND,
    tabs: Vec<Tab>,
    active: usize,
    mintty: String,
    icon: String,
    wsl: String,
    distros: Vec<String>, // default distribution first
    rc: String,
    home: String,
    dpi: u32,
    font: HFONT,
    hover: Hit,
    tracking: bool,
}

// Single-threaded Win32 UI; the window procedure is the only user.
static mut APP: *mut App = null_mut();

fn app() -> &'static mut App {
    unsafe { &mut *APP }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn scale(v: i32) -> i32 {
    v * app().dpi as i32 / 96
}

fn reg_string(key: HKEY, sub: &str, val: &str) -> Option<String> {
    let mut buf = [0u16; 520];
    let mut len = (buf.len() * 2) as u32;
    let sub = wide(sub);
    let val = wide(val);
    let ok = unsafe {
        RegGetValueW(key, sub.as_ptr(), val.as_ptr(), RRF_RT_REG_SZ, null_mut(), buf.as_mut_ptr() as _, &mut len)
    } == ERROR_SUCCESS;
    ok.then(|| {
        let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        String::from_utf16_lossy(&buf[..end])
    })
}

/// Installed WSL distributions (default first), read from the registry
/// so no wsl.exe process is needed.
fn wsl_distros() -> Vec<String> {
    let mut out = Vec::new();
    unsafe {
        let base = wide(r"Software\Microsoft\Windows\CurrentVersion\Lxss");
        let mut key: HKEY = null_mut();
        if RegOpenKeyExW(HKEY_CURRENT_USER, base.as_ptr(), 0, KEY_READ, &mut key) != ERROR_SUCCESS {
            return out;
        }
        let default = reg_string(key, "", "DefaultDistribution").unwrap_or_default();
        for i in 0.. {
            let mut name = [0u16; 128];
            let mut len = name.len() as u32;
            if RegEnumKeyExW(key, i, name.as_mut_ptr(), &mut len, null(), null_mut(), null_mut(), null_mut())
                != ERROR_SUCCESS
            {
                break;
            }
            let sub = String::from_utf16_lossy(&name[..len as usize]);
            match reg_string(key, &sub, "DistributionName") {
                Some(d) if d.starts_with("docker-desktop") => {}
                Some(d) if sub.eq_ignore_ascii_case(&default) => out.insert(0, d),
                Some(d) => out.push(d),
                None => {}
            }
        }
        RegCloseKey(key);
    }
    out
}

fn git_dir() -> String {
    if let Some(p) = reg_string(HKEY_LOCAL_MACHINE, "SOFTWARE\\GitForWindows", "InstallPath") {
        return p;
    }
    for p in [r"C:\Program Files\Git", r"C:\Program Files (x86)\Git"] {
        if Path::new(p).join(r"usr\bin\mintty.exe").exists() {
            return p.into();
        }
    }
    r"C:\Program Files\Git".into()
}

fn dlog(s: &str) {
    use std::io::Write;
    if std::env::var_os("BASHTAB_LOG").is_some() {
        let p = Path::new(&std::env::var("LOCALAPPDATA").unwrap_or_default()).join("bashtab").join("debug.log");
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(p) {
            let _ = writeln!(f, "{s}");
        }
    }
}

fn message_box(text: &str) {
    unsafe {
        MessageBoxW(null_mut(), wide(text).as_ptr(), wide("bashtab").as_ptr(), MB_ICONERROR);
    }
}

// ---- geometry -------------------------------------------------------------

fn client_rect() -> RECT {
    unsafe {
        let mut rc: RECT = zeroed();
        GetClientRect(app().main, &mut rc);
        rc
    }
}

fn bar_h() -> i32 {
    scale(34)
}

fn plus_w() -> i32 {
    scale(36)
}

fn menu_w() -> i32 {
    scale(22)
}

fn tab_w() -> i32 {
    let n = app().tabs.len().max(1) as i32;
    ((client_rect().right - plus_w() - menu_w()) / n).clamp(scale(60), scale(200))
}

fn tab_rect(i: usize) -> RECT {
    let w = tab_w();
    RECT { left: i as i32 * w, top: 0, right: (i as i32 + 1) * w, bottom: bar_h() }
}

fn close_rect(r: &RECT) -> RECT {
    let s = scale(20);
    let cy = (r.top + r.bottom) / 2;
    let right = r.right - scale(8);
    RECT { left: right - s, top: cy - s / 2, right, bottom: cy + s / 2 }
}

fn plus_rect() -> RECT {
    let x = app().tabs.len() as i32 * tab_w();
    RECT { left: x, top: 0, right: x + plus_w(), bottom: bar_h() }
}

fn menu_rect() -> RECT {
    let x = plus_rect().right;
    RECT { left: x, top: 0, right: x + menu_w(), bottom: bar_h() }
}

fn inside(r: &RECT, x: i32, y: i32) -> bool {
    x >= r.left && x < r.right && y >= r.top && y < r.bottom
}

fn hit(x: i32, y: i32) -> Hit {
    if y >= bar_h() {
        return Hit::None;
    }
    for i in 0..app().tabs.len() {
        let r = tab_rect(i);
        if inside(&r, x, y) {
            return if inside(&close_rect(&r), x, y) { Hit::Close(i) } else { Hit::Tab(i) };
        }
    }
    if inside(&plus_rect(), x, y) {
        return Hit::Plus;
    }
    if inside(&menu_rect(), x, y) {
        return Hit::Menu;
    }
    Hit::None
}

// ---- tabs -----------------------------------------------------------------

fn spawn_tab(profile: Profile) {
    let a = app();
    let mut cmd = Command::new(&a.mintty);
    cmd.args(["--nodaemon", "-w", "hide", "-B", "void", "-o", "SwitchShortcuts=no", "-o", "CtrlShiftShortcuts=yes"])
        .args(["-t", profile.label()])
        .current_dir(&a.home);
    match &profile {
        Profile::GitBash => {
            cmd.args(["-i", &a.icon, "/usr/bin/bash", "--noprofile", "--rcfile", &a.rc, "-i"])
                .env("MSYSTEM", "MINGW64")
                .env("CHERE_INVOKING", "1");
        }
        Profile::Wsl(distro) => {
            // -h error: keep the tab open to show why WSL failed to start.
            // "~" is quoted so mintty's cygwin command-line globbing leaves it alone.
            cmd.args(["-h", "error", "-i", &a.wsl, &a.wsl, "-d", distro, "--cd"]).raw_arg("\"~\"");
        }
    }
    match cmd.spawn() {
        Ok(child) => {
            dlog(&format!("spawned pid {} ({})", child.id(), profile.label()));
            let title = profile.label().to_string();
            a.tabs.push(Tab { profile, child, hwnd: null_mut(), title, started: Instant::now() });
            a.active = a.tabs.len() - 1;
            layout();
            refresh();
        }
        Err(e) => message_box(&format!("mintty 실행 실패: {e}")),
    }
}

unsafe extern "system" fn find_cb(hwnd: HWND, lp: LPARAM) -> BOOL {
    let data = &mut *(lp as *mut (u32, HWND));
    let mut pid = 0;
    GetWindowThreadProcessId(hwnd, &mut pid);
    if pid == data.0 && GetWindow(hwnd, GW_OWNER).is_null() {
        let mut cls = [0u16; 32];
        let n = GetClassNameW(hwnd, cls.as_mut_ptr(), cls.len() as i32);
        if String::from_utf16_lossy(&cls[..n.max(0) as usize]) == "mintty" {
            data.1 = hwnd;
            return 0;
        }
    }
    1
}

fn find_mintty_window(pid: u32) -> HWND {
    let mut data: (u32, HWND) = (pid, null_mut());
    unsafe { EnumWindows(Some(find_cb), &mut data as *mut _ as LPARAM) };
    data.1
}

/// Turn mintty's (hidden, borderless) top-level window into a child of ours.
fn adopt(hwnd: HWND) {
    unsafe {
        let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
        let style = (style
            & !(WS_POPUP | WS_CAPTION | WS_THICKFRAME | WS_SYSMENU | WS_MINIMIZEBOX | WS_MAXIMIZEBOX))
            | WS_CHILD;
        SetWindowLongPtrW(hwnd, GWL_STYLE, style as isize);
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32
            & !(WS_EX_APPWINDOW | WS_EX_WINDOWEDGE | WS_EX_CLIENTEDGE | WS_EX_DLGMODALFRAME);
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex as isize);
        SetParent(hwnd, app().main);
    }
}

fn window_title(hwnd: HWND) -> String {
    let mut buf = [0u16; 512];
    let n = unsafe { GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32) };
    // mintty pads its title with spaces
    String::from_utf16_lossy(&buf[..n.max(0) as usize]).trim_end().to_string()
}

fn layout() {
    let a = app();
    let rc = client_rect();
    let top = bar_h();
    let windows: Vec<(HWND, bool)> =
        a.tabs.iter().enumerate().filter(|(_, t)| !t.hwnd.is_null()).map(|(i, t)| (t.hwnd, i == a.active)).collect();
    unsafe {
        for &(h, active) in &windows {
            if !active {
                ShowWindow(h, SW_HIDE);
            }
        }
        for &(h, active) in &windows {
            if active {
                SetWindowPos(
                    h,
                    null_mut(),
                    0,
                    top,
                    rc.right,
                    (rc.bottom - top).max(0),
                    SWP_NOZORDER | SWP_NOACTIVATE | SWP_SHOWWINDOW | SWP_FRAMECHANGED,
                );
            }
        }
    }
}

fn focus_active() {
    let a = app();
    if let Some(t) = a.tabs.get(a.active) {
        if !t.hwnd.is_null() {
            unsafe { SetFocus(t.hwnd) };
        }
    }
}

fn activate(i: usize) {
    let a = app();
    if i >= a.tabs.len() {
        return;
    }
    a.active = i;
    layout();
    focus_active();
    refresh();
}

fn spawn_default_wsl() {
    let a = app();
    a.distros = wsl_distros();
    match a.distros.first().cloned() {
        Some(d) => spawn_tab(Profile::Wsl(d)),
        None => message_box("설치된 WSL 배포판이 없습니다.\n`wsl --install`로 설치할 수 있습니다."),
    }
}

/// Profile picker shown under the ▾ button.
fn show_menu() {
    let a = app();
    a.distros = wsl_distros();
    unsafe {
        let m = CreatePopupMenu();
        AppendMenuW(m, MF_STRING, MENU_GIT, wide("Git Bash\tCtrl+Shift+T").as_ptr());
        if !a.distros.is_empty() {
            AppendMenuW(m, MF_SEPARATOR, 0, null());
        }
        for (i, d) in a.distros.iter().enumerate() {
            let label = if i == 0 { format!("WSL: {d}\tCtrl+Shift+L") } else { format!("WSL: {d}") };
            AppendMenuW(m, MF_STRING, MENU_WSL + i, wide(&label).as_ptr());
        }
        let r = menu_rect();
        let mut pt = POINT { x: r.left, y: r.bottom };
        ClientToScreen(a.main, &mut pt);
        let cmd = TrackPopupMenu(m, TPM_RETURNCMD | TPM_LEFTALIGN | TPM_TOPALIGN, pt.x, pt.y, 0, a.main, null()) as usize;
        DestroyMenu(m);
        if cmd == MENU_GIT {
            spawn_tab(Profile::GitBash);
        } else if cmd >= MENU_WSL {
            if let Some(d) = a.distros.get(cmd - MENU_WSL).cloned() {
                spawn_tab(Profile::Wsl(d));
            }
        }
    }
}

fn cycle(delta: isize) {
    let a = app();
    let n = a.tabs.len() as isize;
    if n > 0 {
        activate(((a.active as isize + delta).rem_euclid(n)) as usize);
    }
}

fn close_tab(i: usize) {
    if let Some(t) = app().tabs.get_mut(i) {
        if t.hwnd.is_null() {
            let _ = t.child.kill();
        } else {
            // lets mintty ask for confirmation if something is still running
            unsafe { PostMessageW(t.hwnd, WM_CLOSE, 0, 0) };
        }
    }
}

/// Adopt newly created mintty windows, track titles, drop exited tabs.
fn poll() {
    let a = app();
    let mut changed = false;
    let mut relayout = false;

    for t in a.tabs.iter_mut() {
        if t.hwnd.is_null() {
            let h = find_mintty_window(t.child.id());
            if !h.is_null() {
                dlog(&format!("adopt pid {} hwnd {:?}", t.child.id(), h));
                adopt(h);
                t.hwnd = h;
                t.title = window_title(h);
                relayout = true;
                changed = true;
            } else if t.started.elapsed().as_secs() > 60 {
                let _ = t.child.kill();
            }
        } else {
            let title = window_title(t.hwnd);
            if title != t.title {
                t.title = title;
                changed = true;
            }
        }
    }

    let mut i = 0;
    while i < a.tabs.len() {
        let t = &mut a.tabs[i];
        let dead = matches!(t.child.try_wait(), Ok(Some(_)))
            || (!t.hwnd.is_null() && unsafe { IsWindow(t.hwnd) } == 0);
        if dead {
            dlog(&format!("tab {i} dead: exit={:?} hwnd_null={}", t.child.try_wait(), t.hwnd.is_null()));
            a.tabs.remove(i);
            if i < a.active {
                a.active -= 1;
            }
            changed = true;
            relayout = true;
        } else {
            i += 1;
        }
    }

    if a.tabs.is_empty() {
        unsafe { DestroyWindow(a.main) };
        return;
    }
    a.active = a.active.min(a.tabs.len() - 1);
    if relayout {
        layout();
        focus_active();
    }
    if changed {
        refresh();
    }
}

// ---- painting -------------------------------------------------------------

fn refresh() {
    let a = app();
    unsafe {
        InvalidateRect(a.main, null(), 0);
        if let Some(t) = a.tabs.get(a.active) {
            SetWindowTextW(a.main, wide(&t.title).as_ptr());
        }
    }
}

fn update_font() {
    let a = app();
    unsafe {
        if !a.font.is_null() {
            DeleteObject(a.font as _);
        }
        a.font = CreateFontW(
            -scale(12), 0, 0, 0, 400, 0, 0, 0,
            DEFAULT_CHARSET as _, OUT_DEFAULT_PRECIS as _, CLIP_DEFAULT_PRECIS as _,
            CLEARTYPE_QUALITY as _, (DEFAULT_PITCH | FF_DONTCARE) as _,
            wide("Segoe UI").as_ptr(),
        );
    }
}

unsafe fn fill(hdc: HDC, r: &RECT, color: u32) {
    let b = CreateSolidBrush(color);
    FillRect(hdc, r, b);
    DeleteObject(b as _);
}

unsafe fn text(hdc: HDC, s: &str, r: &RECT, color: u32, fmt: u32) {
    let w: Vec<u16> = s.encode_utf16().collect();
    let mut r = *r;
    SetTextColor(hdc, color);
    DrawTextW(hdc, w.as_ptr() as _, w.len() as i32, &mut r, fmt);
}

unsafe fn paint(hwnd: HWND) {
    let a = app();
    let mut ps: PAINTSTRUCT = zeroed();
    let hdc = BeginPaint(hwnd, &mut ps);
    let rc = client_rect();
    let bh = bar_h();
    let one_line = DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX;

    // tab bar, double-buffered
    let mem = CreateCompatibleDC(hdc);
    let bmp = CreateCompatibleBitmap(hdc, rc.right.max(1), bh);
    let old_bmp = SelectObject(mem, bmp as _);
    let old_font = SelectObject(mem, a.font as _);
    SetBkMode(mem, TRANSPARENT as _);
    fill(mem, &RECT { left: 0, top: 0, right: rc.right, bottom: bh }, C_BAR);

    for (i, t) in a.tabs.iter().enumerate() {
        let r = tab_rect(i);
        let active = i == a.active;
        if active {
            fill(mem, &r, C_ACTIVE);
            fill(mem, &RECT { bottom: scale(2), ..r }, C_ACCENT);
        } else if a.hover == Hit::Tab(i) || a.hover == Hit::Close(i) {
            fill(mem, &r, C_HOVER);
        } else {
            fill(mem, &RECT { left: r.right - 1, top: scale(9), bottom: bh - scale(9), ..r }, C_SEP);
        }
        let cr = close_rect(&r);
        if a.hover == Hit::Close(i) {
            fill(mem, &cr, C_CLOSE_HOVER);
        }
        let title = if t.hwnd.is_null() { "시작 중…" } else if t.title.is_empty() { t.profile.label() } else { &t.title };
        // profile dot: Git Bash orange, WSL purple
        let dot = scale(6);
        let cy = (r.top + r.bottom) / 2;
        let dx = r.left + scale(12);
        let dot_color = if t.profile == Profile::GitBash { C_ACCENT } else { C_WSL };
        let brush = CreateSolidBrush(dot_color);
        let old_brush = SelectObject(mem, brush as _);
        let old_pen = SelectObject(mem, GetStockObject(NULL_PEN));
        Ellipse(mem, dx, cy - dot / 2, dx + dot + 1, cy + dot / 2 + 1);
        SelectObject(mem, old_pen);
        SelectObject(mem, old_brush);
        DeleteObject(brush as _);
        let tr = RECT { left: dx + dot + scale(8), right: cr.left - scale(4), ..r };
        text(mem, title, &tr, if active { C_TEXT } else { C_DIM }, DT_LEFT | DT_END_ELLIPSIS | one_line);
        text(mem, "×", &cr, if active { C_TEXT } else { C_DIM }, DT_CENTER | one_line);
    }

    let pr = plus_rect();
    if a.hover == Hit::Plus {
        fill(mem, &pr, C_HOVER);
    }
    text(mem, "+", &pr, C_TEXT, DT_CENTER | one_line);

    // ▾ profile menu button, drawn as a small triangle
    let mr = menu_rect();
    if a.hover == Hit::Menu {
        fill(mem, &mr, C_HOVER);
    }
    let (mx, my, s) = ((mr.left + mr.right) / 2, (mr.top + mr.bottom) / 2, scale(4));
    let tri = [POINT { x: mx - s, y: my - s / 2 }, POINT { x: mx + s, y: my - s / 2 }, POINT { x: mx, y: my + s / 2 + 1 }];
    let brush = CreateSolidBrush(C_DIM);
    let old_brush = SelectObject(mem, brush as _);
    let old_pen = SelectObject(mem, GetStockObject(NULL_PEN));
    Polygon(mem, tri.as_ptr(), 3);
    SelectObject(mem, old_pen);
    SelectObject(mem, old_brush);
    DeleteObject(brush as _);

    BitBlt(hdc, 0, 0, rc.right, bh, mem, 0, 0, SRCCOPY);
    SelectObject(mem, old_font);
    SelectObject(mem, old_bmp);
    DeleteObject(bmp as _);
    DeleteDC(mem);

    // terminal area (mintty covers it once adopted; WS_CLIPCHILDREN keeps it intact)
    let body = RECT { left: 0, top: bh, right: rc.right, bottom: rc.bottom };
    fill(hdc, &body, C_TERM_BG);
    if a.tabs.get(a.active).is_some_and(|t| t.hwnd.is_null()) {
        let old = SelectObject(hdc, a.font as _);
        SetBkMode(hdc, TRANSPARENT as _);
        text(hdc, "Git Bash 시작 중…", &body, C_DIM, DT_CENTER | one_line);
        SelectObject(hdc, old);
    }
    EndPaint(hwnd, &ps);
}

// ---- input ----------------------------------------------------------------

fn set_hotkeys(on: bool) {
    let h = app().main;
    unsafe {
        if on {
            RegisterHotKey(h, HK_NEW, MOD_CONTROL | MOD_SHIFT | MOD_NOREPEAT, 'T' as u32);
            RegisterHotKey(h, HK_NEW_WSL, MOD_CONTROL | MOD_SHIFT | MOD_NOREPEAT, 'L' as u32);
            RegisterHotKey(h, HK_CLOSE, MOD_CONTROL | MOD_SHIFT | MOD_NOREPEAT, 'W' as u32);
            RegisterHotKey(h, HK_NEXT, MOD_CONTROL, VK_TAB as u32);
            RegisterHotKey(h, HK_PREV, MOD_CONTROL | MOD_SHIFT, VK_TAB as u32);
            for d in 1..=9 {
                RegisterHotKey(h, HK_TAB + d, MOD_ALT | MOD_NOREPEAT, '0' as u32 + d as u32);
            }
        } else {
            for id in [HK_NEW, HK_NEW_WSL, HK_CLOSE, HK_NEXT, HK_PREV] {
                UnregisterHotKey(h, id);
            }
            for d in 1..=9 {
                UnregisterHotKey(h, HK_TAB + d);
            }
        }
    }
}

fn on_hotkey(id: i32) {
    dlog(&format!("hotkey {id}"));
    let a = app();
    match id {
        HK_NEW => spawn_tab(Profile::GitBash),
        HK_NEW_WSL => spawn_default_wsl(),
        HK_CLOSE => close_tab(a.active),
        HK_NEXT => cycle(1),
        HK_PREV => cycle(-1),
        _ if id > HK_TAB && id <= HK_TAB + 9 => {
            let n = (id - HK_TAB) as usize;
            let len = a.tabs.len();
            if len > 0 {
                activate(if n == 9 { len - 1 } else { n - 1 });
            }
        }
        _ => {}
    }
}

fn mouse_xy(lp: LPARAM) -> (i32, i32) {
    ((lp & 0xffff) as i16 as i32, ((lp >> 16) & 0xffff) as i16 as i32)
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let a = app();
    match msg {
        WM_NCCREATE => {
            a.main = hwnd;
            a.dpi = GetDpiForWindow(hwnd).max(96);
        }
        WM_PAINT => {
            paint(hwnd);
            return 0;
        }
        WM_ERASEBKGND => return 1,
        WM_SIZE => {
            layout();
            InvalidateRect(hwnd, null(), 0);
            return 0;
        }
        WM_TIMER if wp == TIMER_POLL => {
            poll();
            return 0;
        }
        WM_ACTIVATE => {
            if wp & 0xffff != WA_INACTIVE as usize {
                set_hotkeys(true);
                focus_active();
                return 0;
            }
            set_hotkeys(false);
        }
        WM_SETFOCUS => {
            focus_active();
            return 0;
        }
        WM_HOTKEY => {
            on_hotkey(wp as i32);
            return 0;
        }
        WM_MOUSEMOVE => {
            let (x, y) = mouse_xy(lp);
            let h = hit(x, y);
            if h != a.hover {
                a.hover = h;
                InvalidateRect(hwnd, null(), 0);
            }
            if !a.tracking {
                let mut tme = TRACKMOUSEEVENT {
                    cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE,
                    hwndTrack: hwnd,
                    dwHoverTime: 0,
                };
                TrackMouseEvent(&mut tme);
                a.tracking = true;
            }
            return 0;
        }
        WM_MOUSELEAVE => {
            a.tracking = false;
            a.hover = Hit::None;
            InvalidateRect(hwnd, null(), 0);
            return 0;
        }
        WM_LBUTTONDOWN => {
            let (x, y) = mouse_xy(lp);
            match hit(x, y) {
                Hit::Tab(i) => activate(i),
                Hit::Close(i) => close_tab(i),
                Hit::Plus => spawn_tab(Profile::GitBash),
                Hit::Menu => show_menu(),
                Hit::None => {}
            }
            return 0;
        }
        WM_RBUTTONUP => {
            let (x, y) = mouse_xy(lp);
            if matches!(hit(x, y), Hit::Plus | Hit::Menu) {
                show_menu();
            }
            return 0;
        }
        WM_LBUTTONDBLCLK => {
            let (x, y) = mouse_xy(lp);
            if y < bar_h() && hit(x, y) == Hit::None {
                spawn_tab(Profile::GitBash);
            }
            return 0;
        }
        WM_MBUTTONUP => {
            let (x, y) = mouse_xy(lp);
            if let Hit::Tab(i) | Hit::Close(i) = hit(x, y) {
                close_tab(i);
            }
            return 0;
        }
        WM_DPICHANGED => {
            a.dpi = ((wp >> 16) & 0xffff) as u32;
            update_font();
            let r = &*(lp as *const RECT);
            SetWindowPos(hwnd, null_mut(), r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_NOZORDER | SWP_NOACTIVATE);
            return 0;
        }
        WM_CLOSE => {
            for t in a.tabs.iter_mut() {
                let _ = t.child.kill();
            }
            DestroyWindow(hwnd);
            return 0;
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            return 0;
        }
        _ => {}
    }
    DefWindowProcW(hwnd, msg, wp, lp)
}

fn main() {
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);

        let git = git_dir();
        let mintty = format!(r"{git}\usr\bin\mintty.exe");
        if !Path::new(&mintty).exists() {
            message_box("Git for Windows(mintty.exe)를 찾을 수 없습니다.\nhttps://gitforwindows.org 에서 설치해 주세요.");
            return;
        }

        let local = std::env::var("LOCALAPPDATA").unwrap_or_else(|_| std::env::temp_dir().to_string_lossy().into());
        let dir = Path::new(&local).join("bashtab");
        let _ = std::fs::create_dir_all(&dir);
        let rc = dir.join("fast-rc.sh");
        let _ = std::fs::write(&rc, FAST_RC.replace("\r\n", "\n"));

        APP = Box::into_raw(Box::new(App {
            main: null_mut(),
            tabs: Vec::new(),
            active: 0,
            mintty,
            icon: format!(r"{git}\mingw64\share\git\git-for-windows.ico").replace('\\', "/"),
            wsl: format!(r"{}\System32\wsl.exe", std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into()))
                .replace('\\', "/"),
            distros: Vec::new(),
            rc: rc.to_string_lossy().replace('\\', "/"),
            home: std::env::var("USERPROFILE").unwrap_or_else(|_| ".".into()),
            dpi: GetDpiForSystem().max(96),
            font: null_mut(),
            hover: Hit::None,
            tracking: false,
        }));

        let hinst = GetModuleHandleW(null());
        let class = wide("BashtabWindow");
        let icon = LoadImageW(hinst, 1 as _, IMAGE_ICON, 0, 0, LR_DEFAULTSIZE | LR_SHARED);
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            style: CS_DBLCLKS,
            lpfnWndProc: Some(wndproc),
            hInstance: hinst,
            hIcon: icon as _,
            hIconSm: icon as _,
            hCursor: LoadCursorW(null_mut(), IDC_ARROW),
            lpszClassName: class.as_ptr(),
            ..zeroed()
        };
        RegisterClassExW(&wc);

        let hwnd = CreateWindowExW(
            0,
            class.as_ptr(),
            wide("Git Bash").as_ptr(),
            WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            scale(1100),
            scale(700),
            null_mut(),
            null_mut(),
            hinst,
            null(),
        );
        if hwnd.is_null() {
            message_box("창을 만들 수 없습니다.");
            return;
        }
        let dark: i32 = 1;
        DwmSetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE as _, &dark as *const i32 as _, 4);

        update_font();
        spawn_tab(Profile::GitBash);
        ShowWindow(hwnd, SW_SHOW);
        SetTimer(hwnd, TIMER_POLL, 100, None);

        let mut msg: MSG = zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}
