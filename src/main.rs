#![windows_subsystem = "windows"]

//! bashtab: a tab strip that hosts Git for Windows' own mintty windows.
//! Each terminal tab is a real mintty process whose window is re-parented
//! into ours; the home tab is drawn by us. The system title bar is replaced
//! by the tab strip (Chrome style).

use std::mem::{size_of, zeroed};
use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::ptr::{null, null_mut};
use std::time::Instant;

use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Dwm::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::Com::*;
use windows_sys::Win32::System::Diagnostics::ToolHelp::*;
use windows_sys::Win32::System::JobObjects::*;
use windows_sys::Win32::System::Threading::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Registry::*;
use windows_sys::Win32::UI::HiDpi::*;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
use windows_sys::Win32::UI::Shell::*;
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
const MENU_HOME: usize = 2;
const MENU_REMOVE: usize = 3;
const MENU_RESTART: usize = 4;
const MENU_KILL: usize = 5;
const MENU_CLOSE: usize = 6;
const MENU_WSL: usize = 100; // MENU_WSL + distro index
const HK_TAB: i32 = 10; // HK_TAB + 1..=9
// not exported by windows-sys
const WM_MOUSELEAVE: u32 = 0x02A3;
const WM_NCMOUSELEAVE: u32 = 0x02A2;

const fn rgb(r: u8, g: u8, b: u8) -> u32 {
    r as u32 | (g as u32) << 8 | (b as u32) << 16
}
const C_BAR: u32 = rgb(32, 32, 32);
const C_ACTIVE: u32 = rgb(14, 14, 16); // active tab + home background
const C_HOVER: u32 = rgb(46, 46, 50);
const C_CLOSE_HOVER: u32 = rgb(80, 80, 84);
const C_CAPTION_CLOSE: u32 = rgb(196, 43, 28);
const C_SEP: u32 = rgb(62, 62, 66);
const C_ACCENT: u32 = rgb(241, 78, 50);
const C_TEXT: u32 = rgb(240, 240, 240);
const C_DIM: u32 = rgb(150, 150, 156);
const C_FAINT: u32 = rgb(100, 100, 106);
const C_TERM_BG: u32 = rgb(0, 0, 0);
const C_WSL: u32 = rgb(170, 110, 230);
const C_CARD: u32 = rgb(28, 28, 32);
const C_CARD_HOVER: u32 = rgb(40, 40, 46);
const C_CARD_EDGE: u32 = rgb(48, 48, 54);

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

/// A home tab has no profile/child; a terminal tab has both.
struct Tab {
    profile: Option<Profile>,
    child: Option<Child>,
    hwnd: HWND, // mintty window, null until it shows up (always null for home)
    // Job object holding mintty and everything started in the tab, so a
    // force-kill also takes down native programs (java, node, …) that
    // msys signals don't reach. Null for home tabs.
    job: HANDLE,
    title: String,
    started: Instant,
}

impl Tab {
    fn home() -> Tab {
        Tab {
            profile: None,
            child: None,
            hwnd: null_mut(),
            job: null_mut(),
            title: "새 탭".into(),
            started: Instant::now(),
        }
    }
    fn is_home(&self) -> bool {
        self.profile.is_none()
    }
    /// Kill mintty and every process started from this tab.
    fn kill_tree(&mut self) {
        if !self.job.is_null() {
            unsafe { TerminateJobObject(self.job, 1) };
        } else if let Some(c) = self.child.as_mut() {
            let _ = c.kill();
        }
    }
}

impl Drop for Tab {
    fn drop(&mut self) {
        if !self.job.is_null() {
            unsafe { CloseHandle(self.job) };
        }
    }
}

/// Resume every thread of a process created with CREATE_SUSPENDED.
fn resume_process(pid: u32) {
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
        if snap == INVALID_HANDLE_VALUE {
            return;
        }
        let mut te: THREADENTRY32 = zeroed();
        te.dwSize = size_of::<THREADENTRY32>() as u32;
        let mut ok = Thread32First(snap, &mut te);
        while ok != 0 {
            if te.th32OwnerProcessID == pid {
                let th = OpenThread(THREAD_SUSPEND_RESUME, 0, te.th32ThreadID);
                if !th.is_null() {
                    ResumeThread(th);
                    CloseHandle(th);
                }
            }
            ok = Thread32Next(snap, &mut te);
        }
        CloseHandle(snap);
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Hit {
    None,
    Tab(usize),
    Close(usize),
    Plus,
    Menu,
    Min,
    Max,
    CloseWin,
    Home(usize), // index into App::home_items
}

#[derive(Clone)]
enum HomeItem {
    Profile(Profile),
    Fav(usize),
    AddFav,
}

struct Fonts {
    ui: HFONT,
    bold: HFONT,
    title: HFONT,
    small: HFONT,
    mono: HFONT,
    icons: HFONT,
}

struct App {
    main: HWND,
    tabs: Vec<Tab>,
    active: usize,
    mintty: String,
    wsl: String,
    distros: Vec<String>, // default distribution first
    favorites: Vec<String>,
    rc: String,
    home: String,
    data_dir: PathBuf,
    dpi: u32,
    fonts: Fonts,
    icon_face: &'static str,
    hover: Hit,
    pressed: Hit,
    tracking: bool,
    tracking_nc: bool,
    closing: bool, // window close requested, waiting for tabs to exit
    focused: bool,
    home_items: Vec<(RECT, HomeItem)>, // rebuilt on every paint of the home tab
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

/// Git for Windows root: installer registry key, common install dirs,
/// Scoop's PortableGit, then any `git.exe` on PATH (…\cmd or …\bin).
fn git_dir() -> String {
    let has_mintty = |p: &Path| p.join(r"usr\bin\mintty.exe").exists();
    if let Some(p) = reg_string(HKEY_LOCAL_MACHINE, "SOFTWARE\\GitForWindows", "InstallPath") {
        if has_mintty(Path::new(&p)) {
            return p;
        }
    }
    let mut candidates: Vec<PathBuf> = vec![r"C:\Program Files\Git".into(), r"C:\Program Files (x86)\Git".into()];
    if let Ok(home) = std::env::var("USERPROFILE") {
        candidates.push(Path::new(&home).join(r"scoop\apps\git\current"));
    }
    if let Ok(path) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path) {
            if dir.join("git.exe").exists() {
                if let Some(root) = dir.parent() {
                    candidates.push(root.to_path_buf());
                }
            }
        }
    }
    for p in candidates {
        if has_mintty(&p) {
            return p.to_string_lossy().into_owned();
        }
    }
    r"C:\Program Files\Git".into()
}

/// Windows 11 ships "Segoe Fluent Icons"; Windows 10 has "Segoe MDL2 Assets"
/// with the same code points for the caption glyphs.
fn icon_face() -> &'static str {
    let fonts = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\Fonts";
    if reg_string(HKEY_LOCAL_MACHINE, fonts, "Segoe Fluent Icons (TrueType)").is_some() {
        "Segoe Fluent Icons"
    } else {
        "Segoe MDL2 Assets"
    }
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
        MessageBoxW(app().main, wide(text).as_ptr(), wide("bashtab").as_ptr(), MB_ICONERROR);
    }
}

// ---- favorites ------------------------------------------------------------

fn favorites_file() -> PathBuf {
    app().data_dir.join("favorites.txt")
}

fn load_favorites() -> Vec<String> {
    std::fs::read_to_string(favorites_file())
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect()
}

fn save_favorites() {
    let _ = std::fs::write(favorites_file(), app().favorites.join("\n"));
}

fn add_favorite(path: &str) {
    let a = app();
    let path = path.trim_end_matches(['\\', '/']).to_string();
    let path = if path.ends_with(':') { format!("{path}\\") } else { path };
    if Path::new(&path).is_dir() && !a.favorites.iter().any(|f| f.eq_ignore_ascii_case(&path)) {
        a.favorites.push(path);
        save_favorites();
        refresh();
    }
}

fn pick_folder() {
    unsafe {
        let mut name = [0u16; 260];
        let title = wide("즐겨찾기에 추가할 폴더를 고르세요");
        let bi = BROWSEINFOW {
            hwndOwner: app().main,
            pszDisplayName: name.as_mut_ptr(),
            lpszTitle: title.as_ptr(),
            ulFlags: BIF_RETURNONLYFSDIRS | BIF_NEWDIALOGSTYLE,
            ..zeroed()
        };
        let pidl = SHBrowseForFolderW(&bi);
        if pidl.is_null() {
            return;
        }
        let mut buf = [0u16; 1024];
        if SHGetPathFromIDListW(pidl, buf.as_mut_ptr()) != 0 {
            let end = buf.iter().position(|&c| c == 0).unwrap_or(0);
            add_favorite(&String::from_utf16_lossy(&buf[..end]));
        }
        CoTaskMemFree(pidl as _);
    }
}

fn folder_name(path: &str) -> &str {
    let t = path.trim_end_matches(['\\', '/']);
    match t.rfind(['\\', '/']) {
        Some(i) if i + 1 < t.len() => &t[i + 1..],
        _ => path,
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

fn is_maximized() -> bool {
    unsafe { IsZoomed(app().main) != 0 }
}

/// Thickness of the (now client-side) top resize border.
fn frame_thickness() -> i32 {
    unsafe {
        let dpi = app().dpi;
        GetSystemMetricsForDpi(SM_CYFRAME, dpi) + GetSystemMetricsForDpi(SM_CXPADDEDBORDER, dpi)
    }
}

fn bar_h() -> i32 {
    scale(40)
}

/// Gap above the tabs; it stays draggable like a title bar.
fn tab_top() -> i32 {
    if is_maximized() { scale(4) } else { scale(8) }
}

fn caption_w() -> i32 {
    scale(46)
}

fn plus_w() -> i32 {
    scale(34)
}

fn menu_w() -> i32 {
    scale(22)
}

fn tabs_left() -> i32 {
    scale(8)
}

fn tab_w() -> i32 {
    let n = app().tabs.len().max(1) as i32;
    let avail = client_rect().right - tabs_left() - plus_w() - menu_w() - 3 * caption_w() - scale(48);
    (avail / n).clamp(scale(72), scale(220))
}

fn tab_rect(i: usize) -> RECT {
    let w = tab_w();
    let x = tabs_left() + i as i32 * w;
    RECT { left: x, top: tab_top(), right: x + w, bottom: bar_h() }
}

fn close_rect(r: &RECT) -> RECT {
    let s = scale(20);
    let cy = (r.top + r.bottom) / 2;
    let right = r.right - scale(8);
    RECT { left: right - s, top: cy - s / 2, right, bottom: cy + s / 2 }
}

fn plus_rect() -> RECT {
    let x = tabs_left() + app().tabs.len() as i32 * tab_w() + scale(4);
    let cy = (tab_top() + bar_h()) / 2;
    let h = scale(28);
    RECT { left: x, top: cy - h / 2, right: x + plus_w(), bottom: cy + h / 2 }
}

fn menu_rect() -> RECT {
    let p = plus_rect();
    RECT { left: p.right, right: p.right + menu_w(), ..p }
}

/// Minimize, maximize/restore, close: right-aligned, full bar height.
fn caption_rect(n: i32) -> RECT {
    let right = client_rect().right - (2 - n) * caption_w();
    RECT { left: right - caption_w(), top: 0, right, bottom: bar_h() }
}

fn inside(r: &RECT, x: i32, y: i32) -> bool {
    x >= r.left && x < r.right && y >= r.top && y < r.bottom
}

fn hit(x: i32, y: i32) -> Hit {
    let a = app();
    if y >= bar_h() {
        if a.tabs.get(a.active).is_some_and(Tab::is_home) {
            if let Some(i) = a.home_items.iter().position(|(r, _)| inside(r, x, y)) {
                return Hit::Home(i);
            }
        }
        return Hit::None;
    }
    for (n, h) in [(0, Hit::Min), (1, Hit::Max), (2, Hit::CloseWin)] {
        if inside(&caption_rect(n), x, y) {
            return h;
        }
    }
    for i in 0..a.tabs.len() {
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

/// Start a terminal. `slot` is a home tab to turn into the terminal;
/// with `None` a new tab is appended.
fn start_term(slot: Option<usize>, profile: Profile, cwd: Option<&str>) {
    let a = app();
    let mut cmd = Command::new(&a.mintty);
    // started suspended so it joins the tab's job before it can spawn anything
    cmd.creation_flags(CREATE_SUSPENDED);
    cmd.args(["--nodaemon", "-w", "hide", "-B", "void", "-o", "SwitchShortcuts=no", "-o", "CtrlShiftShortcuts=yes"])
        .args(["-t", profile.label()]);
    match &profile {
        Profile::GitBash => {
            cmd.current_dir(cwd.unwrap_or(&a.home))
                .args(["/usr/bin/bash", "--noprofile", "--rcfile", &a.rc, "-i"])
                .env("MSYSTEM", "MINGW64")
                .env("CHERE_INVOKING", "1");
        }
        Profile::Wsl(distro) => {
            // -h error: keep the tab open to show why WSL failed to start.
            // The --cd value is quoted so mintty's cygwin command-line globbing
            // leaves it alone; forward slashes avoid a trailing \" escape.
            let dir = cwd.map(|c| c.replace('\\', "/")).unwrap_or_else(|| "~".into());
            cmd.current_dir(&a.home)
                .args(["-h", "error", &a.wsl, "-d", distro, "--cd"])
                .raw_arg(format!("\"{dir}\""));
        }
    }
    match cmd.spawn() {
        Ok(child) => {
            let job = unsafe {
                let job = CreateJobObjectW(null(), null());
                if !job.is_null() && AssignProcessToJobObject(job, child.as_raw_handle() as HANDLE) == 0 {
                    CloseHandle(job);
                    null_mut()
                } else {
                    job
                }
            };
            resume_process(child.id());
            dlog(&format!("spawned pid {} ({}) cwd={cwd:?} job={}", child.id(), profile.label(), !job.is_null()));
            let tab = Tab {
                title: profile.label().to_string(),
                profile: Some(profile),
                child: Some(child),
                hwnd: null_mut(),
                job,
                started: Instant::now(),
            };
            match slot.filter(|&i| a.tabs.get(i).is_some_and(Tab::is_home)) {
                Some(i) => {
                    a.tabs[i] = tab;
                    a.active = i;
                }
                None => {
                    a.tabs.push(tab);
                    a.active = a.tabs.len() - 1;
                }
            }
            layout();
            refresh();
        }
        Err(e) => message_box(&format!("mintty 실행 실패: {e}")),
    }
}

fn spawn_tab(profile: Profile) {
    start_term(None, profile, None);
}

fn new_home_tab() {
    let a = app();
    a.distros = wsl_distros();
    a.tabs.push(Tab::home());
    activate(a.tabs.len() - 1);
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
    let Some(t) = a.tabs.get(a.active) else { return };
    unsafe {
        if !t.hwnd.is_null() {
            SetFocus(t.hwnd);
        } else if GetFocus() != a.main {
            SetFocus(a.main);
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

/// Builds a popup menu, shows it at `pt` (screen) and returns the chosen id.
unsafe fn popup(items: &[(usize, String)], pt: POINT) -> usize {
    let m = CreatePopupMenu();
    for (id, label) in items {
        if *id == 0 {
            AppendMenuW(m, MF_SEPARATOR, 0, null());
        } else {
            AppendMenuW(m, MF_STRING, *id, wide(label).as_ptr());
        }
    }
    let cmd = TrackPopupMenu(m, TPM_RETURNCMD | TPM_LEFTALIGN | TPM_TOPALIGN, pt.x, pt.y, 0, app().main, null());
    DestroyMenu(m);
    cmd as usize
}

fn wsl_menu_items(items: &mut Vec<(usize, String)>, suffix: &str, first_hint: &str) {
    let a = app();
    if !a.distros.is_empty() {
        items.push((0, String::new()));
    }
    for (i, d) in a.distros.iter().enumerate() {
        let hint = if i == 0 { first_hint } else { "" };
        items.push((MENU_WSL + i, format!("WSL: {d}{suffix}{hint}")));
    }
}

/// Profile picker shown under the ▾ button.
fn show_menu() {
    let a = app();
    a.distros = wsl_distros();
    let mut items = vec![(MENU_HOME, "새 탭 (홈)".to_string()), (0, String::new()), (MENU_GIT, "Git Bash\tCtrl+Shift+T".into())];
    wsl_menu_items(&mut items, "", "\tCtrl+Shift+L");
    let r = menu_rect();
    let mut pt = POINT { x: r.left, y: r.bottom };
    let cmd = unsafe {
        ClientToScreen(a.main, &mut pt);
        popup(&items, pt)
    };
    match cmd {
        MENU_HOME => new_home_tab(),
        MENU_GIT => spawn_tab(Profile::GitBash),
        c if c >= MENU_WSL => {
            if let Some(d) = a.distros.get(c - MENU_WSL).cloned() {
                spawn_tab(Profile::Wsl(d));
            }
        }
        _ => {}
    }
}

/// Right-click menu on a favorite folder.
fn favorite_menu(fav: usize) {
    let a = app();
    let Some(path) = a.favorites.get(fav).cloned() else { return };
    let mut items = vec![(MENU_GIT, "Git Bash에서 열기".to_string())];
    wsl_menu_items(&mut items, "에서 열기", "");
    items.push((0, String::new()));
    items.push((MENU_REMOVE, "즐겨찾기에서 제거".into()));
    let cmd = unsafe {
        let mut pt: POINT = zeroed();
        GetCursorPos(&mut pt);
        popup(&items, pt)
    };
    let slot = Some(a.active);
    match cmd {
        MENU_GIT => start_term(slot, Profile::GitBash, Some(&path)),
        MENU_REMOVE => {
            a.favorites.remove(fav);
            save_favorites();
            refresh();
        }
        c if c >= MENU_WSL => {
            if let Some(d) = a.distros.get(c - MENU_WSL).cloned() {
                start_term(slot, Profile::Wsl(d), Some(&path));
            }
        }
        _ => {}
    }
}

fn home_click(i: usize) {
    let a = app();
    let Some((_, item)) = a.home_items.get(i).cloned() else { return };
    let slot = Some(a.active);
    match item {
        HomeItem::Profile(p) => start_term(slot, p, None),
        HomeItem::Fav(f) => {
            if let Some(path) = a.favorites.get(f).cloned() {
                start_term(slot, Profile::GitBash, Some(&path));
            }
        }
        HomeItem::AddFav => pick_folder(),
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
    let a = app();
    let Some(t) = a.tabs.get_mut(i) else { return };
    if t.is_home() {
        remove_tab(i);
    } else if t.hwnd.is_null() {
        t.kill_tree();
    } else {
        // lets mintty hang up bash, or ask first if something is still running
        unsafe { PostMessageW(t.hwnd, WM_CLOSE, 0, 0) };
    }
}

/// Kill everything in the tab and start the same shell again in its place.
fn restart_tab(i: usize) {
    let a = app();
    let Some(t) = a.tabs.get_mut(i) else { return };
    let Some(profile) = t.profile.clone() else { return };
    t.kill_tree();
    a.tabs[i] = Tab::home();
    start_term(Some(i), profile, None);
}

/// Right-click menu on a tab.
fn tab_menu(i: usize) {
    let a = app();
    let Some(t) = a.tabs.get(i) else { return };
    let mut items = Vec::new();
    if !t.is_home() {
        items.push((MENU_RESTART, "다시 시작".to_string()));
        items.push((MENU_KILL, "강제 종료 (실행 중인 프로그램 포함)".into()));
        items.push((0, String::new()));
    }
    items.push((MENU_CLOSE, "탭 닫기\tCtrl+Shift+W".into()));
    let cmd = unsafe {
        let mut pt: POINT = zeroed();
        GetCursorPos(&mut pt);
        popup(&items, pt)
    };
    match cmd {
        MENU_RESTART => restart_tab(i),
        MENU_KILL => {
            if let Some(t) = a.tabs.get_mut(i) {
                t.kill_tree();
            }
        }
        MENU_CLOSE => close_tab(i),
        _ => {}
    }
}

fn remove_tab(i: usize) {
    let a = app();
    a.tabs.remove(i);
    if a.tabs.is_empty() {
        unsafe { DestroyWindow(a.main) };
        return;
    }
    if i < a.active || a.active >= a.tabs.len() {
        a.active = a.active.saturating_sub(1);
    }
    activate(a.active);
}

/// Adopt newly created mintty windows, track titles, drop exited tabs.
fn poll() {
    let a = app();
    let mut changed = false;
    let mut relayout = false;

    for t in a.tabs.iter_mut() {
        let Some(child) = t.child.as_mut() else { continue };
        if t.hwnd.is_null() {
            let h = find_mintty_window(child.id());
            if !h.is_null() {
                dlog(&format!("adopt pid {} hwnd {:?}", child.id(), h));
                adopt(h);
                t.hwnd = h;
                t.title = window_title(h);
                relayout = true;
                changed = true;
            } else if t.started.elapsed().as_secs() > 60 {
                let _ = child.kill();
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
        let dead = t.child.as_mut().is_some_and(|c| matches!(c.try_wait(), Ok(Some(_))))
            || (!t.hwnd.is_null() && unsafe { IsWindow(t.hwnd) } == 0);
        if dead {
            dlog(&format!("tab {i} dead"));
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
        let title = match a.tabs.get(a.active) {
            Some(t) if !t.is_home() => t.title.as_str(),
            _ => "bashtab",
        };
        SetWindowTextW(a.main, wide(title).as_ptr());
    }
}

unsafe fn make_font(size: i32, weight: i32, face: &str) -> HFONT {
    CreateFontW(
        -scale(size), 0, 0, 0, weight, 0, 0, 0,
        DEFAULT_CHARSET as _, OUT_DEFAULT_PRECIS as _, CLIP_DEFAULT_PRECIS as _,
        CLEARTYPE_QUALITY as _, (DEFAULT_PITCH | FF_DONTCARE) as _,
        wide(face).as_ptr(),
    )
}

fn update_fonts() {
    let a = app();
    unsafe {
        let f = &a.fonts;
        for h in [f.ui, f.bold, f.title, f.small, f.mono, f.icons] {
            if !h.is_null() {
                DeleteObject(h as _);
            }
        }
        a.fonts = Fonts {
            ui: make_font(12, 400, "Segoe UI"),
            bold: make_font(14, 600, "Segoe UI"),
            title: make_font(26, 600, "Segoe UI"),
            small: make_font(11, 400, "Segoe UI"),
            mono: make_font(11, 400, "Cascadia Mono"),
            icons: make_font(10, 400, a.icon_face),
        };
    }
}

unsafe fn fill(hdc: HDC, r: &RECT, color: u32) {
    let b = CreateSolidBrush(color);
    FillRect(hdc, r, b);
    DeleteObject(b as _);
}

/// Filled shape with no outline (or with `edge` outline when given).
unsafe fn shape(hdc: HDC, color: u32, edge: Option<u32>, draw: impl FnOnce(HDC)) {
    let brush = CreateSolidBrush(color);
    let pen = match edge {
        Some(c) => CreatePen(PS_SOLID, 1, c),
        None => null_mut(),
    };
    let old_brush = SelectObject(hdc, brush as _);
    let old_pen = SelectObject(hdc, if pen.is_null() { GetStockObject(NULL_PEN) } else { pen as _ });
    draw(hdc);
    SelectObject(hdc, old_pen);
    SelectObject(hdc, old_brush);
    DeleteObject(brush as _);
    if !pen.is_null() {
        DeleteObject(pen as _);
    }
}

unsafe fn round_rect(hdc: HDC, r: &RECT, radius: i32, color: u32, edge: Option<u32>) {
    shape(hdc, color, edge, |dc| {
        RoundRect(dc, r.left, r.top, r.right, r.bottom, radius, radius);
    });
}

unsafe fn dot(hdc: HDC, x: i32, cy: i32, size: i32, color: u32) {
    shape(hdc, color, None, |dc| {
        Ellipse(dc, x, cy - size / 2, x + size + 1, cy + size / 2 + 1);
    });
}

unsafe fn text(hdc: HDC, font: HFONT, s: &str, r: &RECT, color: u32, fmt: u32) {
    // an empty Vec's pointer is dangling; DrawTextW must not see it
    if s.is_empty() {
        return;
    }
    let w: Vec<u16> = s.encode_utf16().collect();
    let mut r = *r;
    let old = SelectObject(hdc, font as _);
    SetTextColor(hdc, color);
    DrawTextW(hdc, w.as_ptr() as _, w.len() as i32, &mut r, fmt);
    SelectObject(hdc, old);
}

const ONE_LINE: u32 = DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX;

fn profile_color(p: Option<&Profile>) -> u32 {
    match p {
        Some(Profile::GitBash) => C_ACCENT,
        Some(Profile::Wsl(_)) => C_WSL,
        None => C_FAINT,
    }
}

unsafe fn paint_bar(dc: HDC, rc: &RECT) {
    let a = app();
    let bh = bar_h();
    fill(dc, &RECT { left: 0, top: 0, right: rc.right, bottom: bh }, C_BAR);
    let radius = scale(16);

    for (i, t) in a.tabs.iter().enumerate() {
        let r = tab_rect(i);
        let active = i == a.active;
        let hovered = a.hover == Hit::Tab(i) || a.hover == Hit::Close(i);
        if active {
            // rounded top corners; the bottom runs into the body below
            round_rect(dc, &RECT { bottom: r.bottom + radius, ..r }, radius, C_ACTIVE, None);
        } else if hovered {
            let h = RECT { left: r.left + scale(2), right: r.right - scale(2), top: r.top + scale(2), bottom: r.bottom - scale(4) };
            round_rect(dc, &h, scale(12), C_HOVER, None);
        } else if a.active != i + 1 {
            fill(dc, &RECT { left: r.right - 1, top: r.top + scale(9), bottom: r.bottom - scale(9), ..r }, C_SEP);
        }
        let cr = close_rect(&r);
        if a.hover == Hit::Close(i) {
            round_rect(dc, &cr, scale(6), C_CLOSE_HOVER, None);
        }
        let title = if t.is_home() || !t.hwnd.is_null() { t.title.as_str() } else { "시작 중…" };
        let title = if title.is_empty() { t.profile.as_ref().map_or("", Profile::label) } else { title };
        let cy = (r.top + r.bottom) / 2;
        let dx = r.left + scale(12);
        dot(dc, dx, cy, scale(6), profile_color(t.profile.as_ref()));
        let tr = RECT { left: dx + scale(14), right: cr.left - scale(4), ..r };
        let color = if active { C_TEXT } else { C_DIM };
        text(dc, a.fonts.ui, title, &tr, color, DT_LEFT | DT_END_ELLIPSIS | ONE_LINE);
        text(dc, a.fonts.icons, "\u{E8BB}", &cr, color, DT_CENTER | ONE_LINE);
    }

    let pr = plus_rect();
    if a.hover == Hit::Plus {
        round_rect(dc, &pr, scale(8), C_HOVER, None);
    }
    text(dc, a.fonts.icons, "\u{E710}", &pr, C_TEXT, DT_CENTER | ONE_LINE);
    let mr = menu_rect();
    if a.hover == Hit::Menu {
        round_rect(dc, &mr, scale(8), C_HOVER, None);
    }
    text(dc, a.fonts.icons, "\u{E70D}", &mr, C_DIM, DT_CENTER | ONE_LINE);

    // caption buttons
    let glyph = if a.focused { C_TEXT } else { C_FAINT };
    let max_glyph = if is_maximized() { "\u{E923}" } else { "\u{E922}" };
    for (n, h, g) in [(0, Hit::Min, "\u{E921}"), (1, Hit::Max, max_glyph), (2, Hit::CloseWin, "\u{E8BB}")] {
        let r = caption_rect(n);
        let hovered = a.hover == h;
        if hovered {
            fill(dc, &r, if h == Hit::CloseWin { C_CAPTION_CLOSE } else { C_HOVER });
        }
        let c = if hovered && h == Hit::CloseWin { rgb(255, 255, 255) } else { glyph };
        text(dc, a.fonts.icons, g, &r, c, DT_CENTER | ONE_LINE);
    }
}

unsafe fn section(dc: HDC, label: &str, x: i32, y: i32, w: i32) {
    let a = app();
    text(dc, a.fonts.small, label, &RECT { left: x, top: y, right: x + w, bottom: y + scale(20) }, C_DIM, DT_LEFT | ONE_LINE);
}

/// The home tab: profile cards, favorite folders and a shortcut cheat sheet.
/// Clickable areas are recorded in `home_items` for hit testing.
unsafe fn paint_home(dc: HDC, body: &RECT) {
    let a = app();
    a.home_items.clear();
    fill(dc, body, C_ACTIVE);

    let colw = (body.right - body.left - scale(64)).min(scale(760));
    let x0 = (body.left + body.right - colw) / 2;
    let x1 = x0 + colw;
    let mut y = body.top + scale(40);

    text(dc, a.fonts.title, "bashtab", &RECT { left: x0, top: y, right: x1, bottom: y + scale(40) }, C_TEXT, DT_LEFT | ONE_LINE);
    y += scale(40);
    let sub = "새 탭에서 열 셸이나 폴더를 고르세요.";
    text(dc, a.fonts.ui, sub, &RECT { left: x0, top: y, right: x1, bottom: y + scale(22) }, C_DIM, DT_LEFT | ONE_LINE);
    y += scale(42);

    // profiles
    section(dc, "셸", x0, y, colw);
    y += scale(26);
    let mut profiles = vec![Profile::GitBash];
    profiles.extend(a.distros.iter().cloned().map(Profile::Wsl));
    let (cw, ch, gap) = (scale(236), scale(64), scale(12));
    let per_row = ((colw + gap) / (cw + gap)).max(1);
    for (i, p) in profiles.iter().enumerate() {
        let (col, row) = (i as i32 % per_row, i as i32 / per_row);
        let left = x0 + col * (cw + gap);
        let top = y + row * (ch + gap);
        let r = RECT { left, top, right: left + cw, bottom: top + ch };
        let hovered = a.hover == Hit::Home(a.home_items.len());
        round_rect(dc, &r, scale(12), if hovered { C_CARD_HOVER } else { C_CARD }, Some(C_CARD_EDGE));
        let cy1 = top + scale(22);
        dot(dc, left + scale(16), cy1, scale(8), profile_color(Some(p)));
        let name = RECT { left: left + scale(32), top: cy1 - scale(11), right: r.right - scale(12), bottom: cy1 + scale(11) };
        text(dc, a.fonts.bold, p.label(), &name, C_TEXT, DT_LEFT | DT_END_ELLIPSIS | ONE_LINE);
        let (desc, keys) = match p {
            Profile::GitBash => ("Git for Windows · MINGW64", "Ctrl+Shift+T"),
            Profile::Wsl(_) if i == 1 => ("WSL · 기본 배포판", "Ctrl+Shift+L"),
            Profile::Wsl(_) => ("WSL", ""),
        };
        let line2 = RECT { left: left + scale(32), top: top + scale(34), right: r.right - scale(12), bottom: top + scale(54) };
        text(dc, a.fonts.small, desc, &line2, C_DIM, DT_LEFT | DT_END_ELLIPSIS | ONE_LINE);
        text(dc, a.fonts.mono, keys, &name, C_FAINT, DT_RIGHT | ONE_LINE);
        a.home_items.push((r, HomeItem::Profile(p.clone())));
    }
    let rows = (profiles.len() as i32 + per_row - 1) / per_row;
    y += rows * (ch + gap) + scale(20);

    // favorite folders
    section(dc, "즐겨찾는 폴더", x0, y, colw);
    let add = RECT { left: x1 - scale(110), top: y - scale(4), right: x1, bottom: y + scale(24) };
    if a.hover == Hit::Home(a.home_items.len()) {
        round_rect(dc, &add, scale(8), C_HOVER, None);
    }
    text(dc, a.fonts.ui, "+ 폴더 추가", &add, C_TEXT, DT_CENTER | ONE_LINE);
    a.home_items.push((add, HomeItem::AddFav));
    y += scale(28);

    let row_h = scale(44);
    if a.favorites.is_empty() {
        let r = RECT { left: x0, top: y, right: x1, bottom: y + scale(56) };
        round_rect(dc, &r, scale(12), C_ACTIVE, Some(C_CARD_EDGE));
        let hint = "폴더를 여기로 끌어다 놓거나 '+ 폴더 추가'를 누르세요";
        text(dc, a.fonts.ui, hint, &r, C_FAINT, DT_CENTER | ONE_LINE);
        y += scale(56);
    } else {
        for (f, path) in a.favorites.iter().enumerate() {
            let r = RECT { left: x0, top: y, right: x1, bottom: y + row_h - scale(4) };
            let hovered = a.hover == Hit::Home(a.home_items.len());
            round_rect(dc, &r, scale(10), if hovered { C_CARD_HOVER } else { C_CARD }, Some(C_CARD_EDGE));
            let name = RECT { left: r.left + scale(16), right: r.left + scale(16) + scale(200), ..r };
            text(dc, a.fonts.bold, folder_name(path), &name, C_TEXT, DT_LEFT | DT_END_ELLIPSIS | ONE_LINE);
            let full = RECT { left: name.right + scale(12), right: r.right - scale(16), ..r };
            text(dc, a.fonts.small, path, &full, C_DIM, DT_LEFT | DT_PATH_ELLIPSIS | ONE_LINE);
            a.home_items.push((r, HomeItem::Fav(f)));
            y += row_h;
        }
        let hint = "클릭: Git Bash로 열기 · 우클릭: WSL로 열기 / 제거";
        text(dc, a.fonts.small, hint, &RECT { left: x0, top: y, right: x1, bottom: y + scale(20) }, C_FAINT, DT_LEFT | ONE_LINE);
        y += scale(20);
    }
    y += scale(28);

    // shortcuts
    section(dc, "단축키", x0, y, colw);
    y += scale(26);
    let keys = [
        ("Ctrl+Shift+T", "새 Git Bash 탭"),
        ("Ctrl+Shift+L", "새 WSL 탭"),
        ("Ctrl+Shift+W", "탭 닫기"),
        ("Ctrl+Tab", "다음 탭"),
        ("Ctrl+Shift+Tab", "이전 탭"),
        ("Alt+1~9", "N번째 탭"),
        ("Ctrl+Shift+C / V", "복사 / 붙여넣기"),
        ("+ / ▾", "새 탭 / 셸 고르기"),
    ];
    let half = colw / 2;
    for (k, (key, desc)) in keys.iter().enumerate() {
        let left = x0 + (k as i32 % 2) * half;
        let top = y + (k as i32 / 2) * scale(26);
        let kr = RECT { left, top, right: left + scale(140), bottom: top + scale(24) };
        text(dc, a.fonts.mono, key, &kr, C_TEXT, DT_LEFT | ONE_LINE);
        let dr = RECT { left: kr.right, right: left + half - scale(12), ..kr };
        text(dc, a.fonts.ui, desc, &dr, C_DIM, DT_LEFT | DT_END_ELLIPSIS | ONE_LINE);
    }
}

unsafe fn paint(hwnd: HWND) {
    let a = app();
    let mut ps: PAINTSTRUCT = zeroed();
    let hdc = BeginPaint(hwnd, &mut ps);
    let rc = client_rect();
    let bh = bar_h();

    // whole client, double-buffered; WS_CLIPCHILDREN keeps the mintty window intact
    let mem = CreateCompatibleDC(hdc);
    let bmp = CreateCompatibleBitmap(hdc, rc.right.max(1), rc.bottom.max(1));
    let old_bmp = SelectObject(mem, bmp as _);
    SetBkMode(mem, TRANSPARENT as _);

    paint_bar(mem, &rc);
    let body = RECT { left: 0, top: bh, right: rc.right, bottom: rc.bottom };
    match a.tabs.get(a.active) {
        Some(t) if t.is_home() => paint_home(mem, &body),
        Some(t) => {
            fill(mem, &body, C_TERM_BG);
            if t.hwnd.is_null() {
                let msg = format!("{} 시작 중…", t.title);
                text(mem, a.fonts.ui, &msg, &body, C_DIM, DT_CENTER | ONE_LINE);
            }
        }
        None => fill(mem, &body, C_TERM_BG),
    }

    BitBlt(hdc, 0, 0, rc.right, rc.bottom, mem, 0, 0, SRCCOPY);
    SelectObject(mem, old_bmp);
    DeleteObject(bmp as _);
    DeleteDC(mem);
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

fn set_hover(h: Hit) {
    let a = app();
    if h != a.hover {
        a.hover = h;
        unsafe { InvalidateRect(a.main, null(), 0) };
    }
}

fn track_mouse(hwnd: HWND, nonclient: bool) {
    let a = app();
    let flag = if nonclient { &mut a.tracking_nc } else { &mut a.tracking };
    if *flag {
        return;
    }
    let mut tme = TRACKMOUSEEVENT {
        cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
        dwFlags: if nonclient { TME_LEAVE | TME_NONCLIENT } else { TME_LEAVE },
        hwndTrack: hwnd,
        dwHoverTime: 0,
    };
    unsafe { TrackMouseEvent(&mut tme) };
    *flag = true;
}

fn toggle_maximize() {
    unsafe { ShowWindow(app().main, if is_maximized() { SW_RESTORE } else { SW_MAXIMIZE }) };
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let a = app();
    match msg {
        WM_NCCREATE => {
            a.main = hwnd;
            a.dpi = GetDpiForWindow(hwnd).max(96);
        }
        // Drop the system caption; keep the side/bottom resize frame.
        WM_NCCALCSIZE if wp != 0 => {
            let p = &mut *(lp as *mut NCCALCSIZE_PARAMS);
            let top = p.rgrc[0].top;
            DefWindowProcW(hwnd, msg, wp, lp);
            p.rgrc[0].top = top;
            if IsZoomed(hwnd) != 0 {
                // a maximized window hangs over the monitor edge by the frame size
                p.rgrc[0].top += frame_thickness();
            }
            return 0;
        }
        WM_NCHITTEST => {
            let def = DefWindowProcW(hwnd, msg, wp, lp);
            if def != HTCLIENT as LRESULT {
                return def;
            }
            let (sx, sy) = mouse_xy(lp);
            let mut pt = POINT { x: sx, y: sy };
            ScreenToClient(hwnd, &mut pt);
            if !is_maximized() && pt.y < frame_thickness() {
                let corner = frame_thickness() * 2;
                let w = client_rect().right;
                return if pt.x < corner {
                    HTTOPLEFT
                } else if pt.x >= w - corner {
                    HTTOPRIGHT
                } else {
                    HTTOP
                } as LRESULT;
            }
            if pt.y < bar_h() {
                return match hit(pt.x, pt.y) {
                    Hit::None => HTCAPTION,
                    Hit::Max => HTMAXBUTTON, // enables the Windows 11 snap layout flyout
                    _ => HTCLIENT,
                } as LRESULT;
            }
            return HTCLIENT as LRESULT;
        }
        WM_NCMOUSEMOVE => {
            set_hover(if wp == HTMAXBUTTON as usize { Hit::Max } else { Hit::None });
            track_mouse(hwnd, true);
        }
        WM_NCMOUSELEAVE => {
            a.tracking_nc = false;
            if a.hover == Hit::Max {
                set_hover(Hit::None);
            }
        }
        WM_NCLBUTTONDOWN if wp == HTMAXBUTTON as usize => return 0,
        WM_NCLBUTTONUP if wp == HTMAXBUTTON as usize => {
            toggle_maximize();
            return 0;
        }
        WM_GETMINMAXINFO => {
            let mmi = &mut *(lp as *mut MINMAXINFO);
            mmi.ptMinTrackSize = POINT { x: scale(520), y: scale(360) };
            return 0;
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
            a.focused = wp & 0xffff != WA_INACTIVE as usize;
            InvalidateRect(hwnd, null(), 0);
            if a.focused {
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
        WM_SETCURSOR if (lp & 0xffff) as u32 == HTCLIENT => {
            let mut pt: POINT = zeroed();
            GetCursorPos(&mut pt);
            ScreenToClient(hwnd, &mut pt);
            if matches!(hit(pt.x, pt.y), Hit::Home(_)) {
                SetCursor(LoadCursorW(null_mut(), IDC_HAND));
                return 1;
            }
        }
        WM_MOUSEMOVE => {
            let (x, y) = mouse_xy(lp);
            set_hover(hit(x, y));
            track_mouse(hwnd, false);
            return 0;
        }
        WM_MOUSELEAVE => {
            a.tracking = false;
            set_hover(Hit::None);
            return 0;
        }
        WM_LBUTTONDOWN => {
            let (x, y) = mouse_xy(lp);
            let h = hit(x, y);
            a.pressed = h;
            match h {
                Hit::Tab(i) => activate(i),
                Hit::Close(i) => close_tab(i),
                Hit::Plus => new_home_tab(),
                Hit::Menu => show_menu(),
                _ => {}
            }
            return 0;
        }
        WM_LBUTTONUP => {
            let (x, y) = mouse_xy(lp);
            let h = hit(x, y);
            if h == a.pressed {
                match h {
                    Hit::Min => {
                        ShowWindow(hwnd, SW_MINIMIZE);
                    }
                    Hit::Max => toggle_maximize(),
                    Hit::CloseWin => {
                        PostMessageW(hwnd, WM_CLOSE, 0, 0);
                    }
                    Hit::Home(i) => home_click(i),
                    _ => {}
                }
            }
            a.pressed = Hit::None;
            return 0;
        }
        WM_RBUTTONUP => {
            let (x, y) = mouse_xy(lp);
            match hit(x, y) {
                Hit::Plus | Hit::Menu => show_menu(),
                Hit::Tab(i) | Hit::Close(i) => tab_menu(i),
                Hit::Home(i) => {
                    if let Some((_, HomeItem::Fav(f))) = a.home_items.get(i).cloned() {
                        favorite_menu(f);
                    }
                }
                _ => {}
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
        WM_DROPFILES => {
            let drop = wp as HDROP;
            let n = DragQueryFileW(drop, u32::MAX, null_mut(), 0);
            for i in 0..n {
                let mut buf = [0u16; 1024];
                let len = DragQueryFileW(drop, i, buf.as_mut_ptr(), buf.len() as u32) as usize;
                add_favorite(&String::from_utf16_lossy(&buf[..len]));
            }
            DragFinish(drop);
            return 0;
        }
        WM_DPICHANGED => {
            a.dpi = ((wp >> 16) & 0xffff) as u32;
            update_fonts();
            let r = &*(lp as *const RECT);
            SetWindowPos(hwnd, null_mut(), r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_NOZORDER | SWP_NOACTIVATE);
            return 0;
        }
        // First close: ask each mintty to close so it hangs up bash (killing
        // mintty would orphan bash); the window goes away once the poll timer
        // sees every tab gone. Closing again while that is pending forces it.
        WM_CLOSE => {
            if a.closing {
                for t in a.tabs.iter_mut() {
                    t.kill_tree();
                }
                DestroyWindow(hwnd);
                return 0;
            }
            a.closing = true;
            a.tabs.retain(|t| !t.is_home());
            for t in a.tabs.iter_mut() {
                if t.hwnd.is_null() {
                    t.kill_tree();
                } else {
                    PostMessageW(t.hwnd, WM_CLOSE, 0, 0);
                }
            }
            if a.tabs.is_empty() {
                DestroyWindow(hwnd);
            } else {
                a.active = 0;
                layout();
                refresh();
            }
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
        CoInitializeEx(null(), COINIT_APARTMENTTHREADED as _); // folder picker

        let git = git_dir();
        let mintty = format!(r"{git}\usr\bin\mintty.exe");
        if !Path::new(&mintty).exists() {
            MessageBoxW(
                null_mut(),
                wide("Git for Windows(mintty.exe)를 찾을 수 없습니다.\nhttps://gitforwindows.org 에서 설치해 주세요.").as_ptr(),
                wide("bashtab").as_ptr(),
                MB_ICONERROR,
            );
            return;
        }

        let local = std::env::var("LOCALAPPDATA").unwrap_or_else(|_| std::env::temp_dir().to_string_lossy().into());
        let data_dir = Path::new(&local).join("bashtab");
        let _ = std::fs::create_dir_all(&data_dir);
        let rc = data_dir.join("fast-rc.sh");
        let _ = std::fs::write(&rc, FAST_RC.replace("\r\n", "\n"));

        APP = Box::into_raw(Box::new(App {
            main: null_mut(),
            tabs: Vec::new(),
            active: 0,
            mintty,
            wsl: format!(r"{}\System32\wsl.exe", std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into()))
                .replace('\\', "/"),
            distros: wsl_distros(),
            favorites: Vec::new(),
            rc: rc.to_string_lossy().replace('\\', "/"),
            home: std::env::var("USERPROFILE").unwrap_or_else(|_| ".".into()),
            data_dir,
            dpi: GetDpiForSystem().max(96),
            fonts: Fonts {
                ui: null_mut(),
                bold: null_mut(),
                title: null_mut(),
                small: null_mut(),
                mono: null_mut(),
                icons: null_mut(),
            },
            icon_face: icon_face(),
            hover: Hit::None,
            pressed: Hit::None,
            tracking: false,
            tracking_nc: false,
            closing: false,
            focused: true,
            home_items: Vec::new(),
        }));
        app().favorites = load_favorites();

        let hinst = GetModuleHandleW(null());
        let class = wide("BashtabWindow");
        let icon = LoadImageW(hinst, 1 as _, IMAGE_ICON, 0, 0, LR_DEFAULTSIZE | LR_SHARED);
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
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
            wide("bashtab").as_ptr(),
            WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            scale(1100),
            scale(720),
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
        // re-run WM_NCCALCSIZE so the custom frame applies from the start
        SetWindowPos(hwnd, null_mut(), 0, 0, 0, 0, SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER);
        DragAcceptFiles(hwnd, 1);

        update_fonts();
        new_home_tab();
        ShowWindow(hwnd, SW_SHOW);
        SetTimer(hwnd, TIMER_POLL, 100, None);

        let mut msg: MSG = zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}
