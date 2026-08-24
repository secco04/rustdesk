use crate::{
    client::*,
    flutter_ffi::{EventToUI, SessionID},
    ui_session_interface::{io_loop, InvokeUiSession, Session},
};
use flutter_rust_bridge::StreamSink;
#[cfg(not(any(target_os = "android", target_os = "ios")))]
use hbb_common::dlopen::{
    symbor::{Library, Symbol},
    Error as LibError,
};
use hbb_common::{
    anyhow::anyhow, bail, config::LocalConfig, get_version_number, log, message_proto::*,
    rendezvous_proto::ConnType, ResultType,
};
use serde::Serialize;
use serde_json::json;
#[cfg(target_os = "windows")]
use std::io::{Error as IoError, ErrorKind as IoErrorKind};
use std::{
    collections::{HashMap, HashSet},
    ffi::CString,
    os::raw::{c_char, c_int, c_void},
    str::FromStr,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, RwLock,
    },
};

/// tag "main" for [Desktop Main Page] and [Mobile (Client and Server)] (the mobile don't need multiple windows, only one global event stream is needed)
/// tag "cm" only for [Desktop CM Page]
pub(crate) const APP_TYPE_MAIN: &str = "main";
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub(crate) const APP_TYPE_CM: &str = "cm";
#[cfg(any(target_os = "android", target_os = "ios"))]
pub(crate) const APP_TYPE_CM: &str = "main";

// Do not remove the following constants.
// Uncomment them when they are used.
// pub(crate) const APP_TYPE_DESKTOP_REMOTE: &str = "remote";
// pub(crate) const APP_TYPE_DESKTOP_FILE_TRANSFER: &str = "file transfer";
// pub(crate) const APP_TYPE_DESKTOP_PORT_FORWARD: &str = "port forward";

pub type FlutterSession = Arc<Session<FlutterHandler>>;

lazy_static::lazy_static! {
    pub(crate) static ref CUR_SESSION_ID: RwLock<SessionID> = Default::default(); // For desktop only
    static ref GLOBAL_EVENT_STREAM: RwLock<HashMap<String, StreamSink<String>>> = Default::default(); // rust to dart event channel
}

#[cfg(target_os = "windows")]
lazy_static::lazy_static! {
    pub static ref TEXTURE_RGBA_RENDERER_PLUGIN: Result<Library, LibError> = load_plugin_in_app_path("texture_rgba_renderer_plugin.dll");
}

#[cfg(target_os = "linux")]
lazy_static::lazy_static! {
    pub static ref TEXTURE_RGBA_RENDERER_PLUGIN: Result<Library, LibError> = Library::open("libtexture_rgba_renderer_plugin.so");
}

#[cfg(target_os = "macos")]
lazy_static::lazy_static! {
    pub static ref TEXTURE_RGBA_RENDERER_PLUGIN: Result<Library, LibError> = Library::open_self();
}

#[cfg(target_os = "windows")]
lazy_static::lazy_static! {
    pub static ref TEXTURE_GPU_RENDERER_PLUGIN: Result<Library, LibError> = load_plugin_in_app_path("flutter_gpu_texture_renderer_plugin.dll");
}

// Move this function into `src/platform/windows.rs` if there're more calls to load plugins.
// Load dll with full path.
#[cfg(target_os = "windows")]
fn load_plugin_in_app_path(dll_name: &str) -> Result<Library, LibError> {
    match std::env::current_exe() {
        Ok(exe_file) => {
            if let Some(cur_dir) = exe_file.parent() {
                let full_path = cur_dir.join(dll_name);
                if !full_path.exists() {
                    Err(LibError::OpeningLibraryError(IoError::new(
                        IoErrorKind::NotFound,
                        format!("{} not found", dll_name),
                    )))
                } else {
                    Library::open(full_path)
                }
            } else {
                Err(LibError::OpeningLibraryError(IoError::new(
                    IoErrorKind::Other,
                    format!(
                        "Invalid exe parent for {}",
                        exe_file.to_string_lossy().as_ref()
                    ),
                )))
            }
        }
        Err(e) => Err(LibError::OpeningLibraryError(e)),
    }
}

/// FFI for rustdesk core's main entry.
/// Return true if the app should continue running with UI(possibly Flutter), false if the app should exit.
#[cfg(not(windows))]
#[no_mangle]
pub extern "C" fn rustdesk_core_main() -> bool {
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    if crate::core_main::core_main().is_some() {
        return true;
    } else {
        #[cfg(target_os = "macos")]
        std::process::exit(0);
    }
    #[cfg(not(target_os = "macos"))]
    false
}

#[cfg(target_os = "macos")]
#[no_mangle]
pub extern "C" fn handle_applicationShouldOpenUntitledFile() {
    crate::platform::macos::handle_application_should_open_untitled_file();
}

#[cfg(windows)]
#[no_mangle]
pub extern "C" fn rustdesk_core_main_args(args_len: *mut c_int) -> *mut *mut c_char {
    unsafe { std::ptr::write(args_len, 0) };
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        if let Some(args) = crate::core_main::core_main() {
            return rust_args_to_c_args(args, args_len);
        }
        return std::ptr::null_mut() as _;
    }
    #[cfg(any(target_os = "android", target_os = "ios"))]
    return std::ptr::null_mut() as _;
}

// https://gist.github.com/iskakaushik/1c5b8aa75c77479c33c4320913eebef6
#[cfg(windows)]
fn rust_args_to_c_args(args: Vec<String>, outlen: *mut c_int) -> *mut *mut c_char {
    let mut v = vec![];

    // Let's fill a vector with null-terminated strings
    for s in args {
        match CString::new(s) {
            Ok(s) => v.push(s),
            Err(_) => return std::ptr::null_mut() as _,
        }
    }

    // Turning each null-terminated string into a pointer.
    // `into_raw` takes ownershop, gives us the pointer and does NOT drop the data.
    let mut out = v.into_iter().map(|s| s.into_raw()).collect::<Vec<_>>();

    // Make sure we're not wasting space.
    out.shrink_to_fit();
    debug_assert!(out.len() == out.capacity());

    // Get the pointer to our vector.
    let len = out.len();
    let ptr = out.as_mut_ptr();
    std::mem::forget(out);

    // Let's write back the length the caller can expect
    unsafe { std::ptr::write(outlen, len as c_int) };

    // Finally return the data
    ptr
}

#[no_mangle]
pub unsafe extern "C" fn free_c_args(ptr: *mut *mut c_char, len: c_int) {
    let len = len as usize;

    // Get back our vector.
    // Previously we shrank to fit, so capacity == length.
    let v = Vec::from_raw_parts(ptr, len, len);

    // Now drop one string at a time.
    for elem in v {
        let s = CString::from_raw(elem);
        std::mem::drop(s);
    }

    // Afterwards the vector will be dropped and thus freed.
}

#[cfg(windows)]
#[no_mangle]
pub unsafe extern "C" fn get_rustdesk_app_name(buffer: *mut u16, length: i32) -> i32 {
    let name = crate::platform::wide_string(&crate::get_app_name());
    if length > name.len() as i32 {
        std::ptr::copy_nonoverlapping(name.as_ptr(), buffer, name.len());
        return 0;
    }
    -1
}

#[derive(Default)]
struct SessionHandler {
    event_stream: Option<StreamSink<EventToUI>>,
    // displays of current session.
    // We need this variable to check if the display is in use before pushing rgba to flutter.
    displays: Vec<usize>,
    renderer: VideoRenderer,
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum RenderType {
    PixelBuffer,
    #[cfg(feature = "vram")]
    Texture,
}

#[derive(Clone)]
pub struct FlutterHandler {
    // ui session id -> display handler data
    session_handlers: Arc<RwLock<HashMap<SessionID, SessionHandler>>>,
    display_rgbas: Arc<RwLock<HashMap<usize, RgbaData>>>,
    peer_info: Arc<RwLock<PeerInfo>>,
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    hooks: Arc<RwLock<HashMap<String, SessionHook>>>,
    use_texture_render: Arc<AtomicBool>,
}

impl Default for FlutterHandler {
    fn default() -> Self {
        Self {
            session_handlers: Default::default(),
            display_rgbas: Default::default(),
            peer_info: Default::default(),
            #[cfg(not(any(target_os = "android", target_os = "ios")))]
            hooks: Default::default(),
            use_texture_render: Arc::new(
                AtomicBool::new(crate::ui_interface::use_texture_render()),
            ),
        }
    }
}

#[derive(Default, Clone)]
struct RgbaData {
    // SAFETY: [rgba] is guarded by [rgba_valid], and it's safe to reach [rgba] with `rgba_valid == true`.
    // We must check the `rgba_valid` before reading [rgba].
    data: Vec<u8>,
    valid: bool,
}

pub type FlutterRgbaRendererPluginOnRgba = unsafe extern "C" fn(
    texture_rgba: *mut c_void,
    buffer: *const u8,
    len: c_int,
    width: c_int,
    height: c_int,
    dst_rgba_stride: c_int,
);

#[cfg(feature = "vram")]
pub type FlutterGpuTextureRendererPluginCApiSetTexture =
    unsafe extern "C" fn(output: *mut c_void, texture: *mut c_void);

#[cfg(feature = "vram")]
pub type FlutterGpuTextureRendererPluginCApiGetAdapterLuid = unsafe extern "C" fn() -> i64;

pub(super) type TextureRgbaPtr = usize;

struct DisplaySessionInfo {
    // TextureRgba pointer in flutter native.
    texture_rgba_ptr: TextureRgbaPtr,
    size: (usize, usize),
    #[cfg(feature = "vram")]
    gpu_output_ptr: usize,
    notify_render_type: Option<RenderType>,
}

// Video Texture Renderer in Flutter
#[derive(Clone)]
struct VideoRenderer {
    is_support_multi_ui_session: bool,
    map_display_sessions: Arc<RwLock<HashMap<usize, DisplaySessionInfo>>>,
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    on_rgba_func: Option<Symbol<'static, FlutterRgbaRendererPluginOnRgba>>,
    #[cfg(feature = "vram")]
    on_texture_func: Option<Symbol<'static, FlutterGpuTextureRendererPluginCApiSetTexture>>,
}

impl Default for VideoRenderer {
    fn default() -> Self {
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        let on_rgba_func = match &*TEXTURE_RGBA_RENDERER_PLUGIN {
            Ok(lib) => {
                let find_sym_res = unsafe {
                    lib.symbol::<FlutterRgbaRendererPluginOnRgba>("FlutterRgbaRendererPluginOnRgba")
                };
                match find_sym_res {
                    Ok(sym) => Some(sym),
                    Err(e) => {
                        log::error!("Failed to find symbol FlutterRgbaRendererPluginOnRgba, {e}");
                        None
                    }
                }
            }
            Err(e) => {
                log::error!("Failed to load texture rgba renderer plugin, {e}");
                None
            }
        };
        #[cfg(feature = "vram")]
        let on_texture_func = match &*TEXTURE_GPU_RENDERER_PLUGIN {
            Ok(lib) => {
                let find_sym_res = unsafe {
                    lib.symbol::<FlutterGpuTextureRendererPluginCApiSetTexture>(
                        "FlutterGpuTextureRendererPluginCApiSetTexture",
                    )
                };
                match find_sym_res {
                    Ok(sym) => Some(sym),
                    Err(e) => {
                        log::error!("Failed to find symbol FlutterGpuTextureRendererPluginCApiSetTexture, {e}");
                        None
                    }
                }
            }
            Err(e) => {
                log::error!("Failed to load texture gpu renderer plugin, {e}");
                None
            }
        };

        Self {
            map_display_sessions: Default::default(),
            is_support_multi_ui_session: false,
            #[cfg(not(any(target_os = "android", target_os = "ios")))]
            on_rgba_func,
            #[cfg(feature = "vram")]
            on_texture_func,
        }
    }
}

impl VideoRenderer {
    #[inline]
    fn set_size(&mut self, display: usize, width: usize, height: usize) {
        let mut sessions_lock = self.map_display_sessions.write().unwrap();
        if let Some(info) = sessions_lock.get_mut(&display) {
            info.size = (width, height);
            info.notify_render_type = None;
        } else {
            sessions_lock.insert(
                display,
                DisplaySessionInfo {
                    texture_rgba_ptr: usize::default(),
                    size: (width, height),
                    #[cfg(feature = "vram")]
                    gpu_output_ptr: usize::default(),
                    notify_render_type: None,
                },
            );
        }
    }

    fn register_pixelbuffer_texture(&self, display: usize, ptr: usize) {
        let mut sessions_lock = self.map_display_sessions.write().unwrap();
        if ptr == 0 {
            if let Some(info) = sessions_lock.get_mut(&display) {
                if info.texture_rgba_ptr != usize::default() {
                    info.texture_rgba_ptr = usize::default();
                }
                #[cfg(feature = "vram")]
                if info.gpu_output_ptr != usize::default() {
                    return;
                }
            }
            sessions_lock.remove(&display);
        } else {
            if let Some(info) = sessions_lock.get_mut(&display) {
                if info.texture_rgba_ptr != usize::default()
                    && info.texture_rgba_ptr != ptr as TextureRgbaPtr
                {
                    log::warn!(
                        "texture_rgba_ptr is not null and not equal to ptr, replace {} to {}",
                        info.texture_rgba_ptr,
                        ptr
                    );
                }
                info.texture_rgba_ptr = ptr as _;
                info.notify_render_type = None;
            } else {
                if ptr != 0 {
                    sessions_lock.insert(
                        display,
                        DisplaySessionInfo {
                            texture_rgba_ptr: ptr as _,
                            size: (0, 0),
                            #[cfg(feature = "vram")]
                            gpu_output_ptr: usize::default(),
                            notify_render_type: None,
                        },
                    );
                }
            }
        }
    }

    #[cfg(feature = "vram")]
    pub fn register_gpu_output(&self, display: usize, ptr: usize) {
        let mut sessions_lock = self.map_display_sessions.write().unwrap();
        if ptr == 0 {
            if let Some(info) = sessions_lock.get_mut(&display) {
                if info.gpu_output_ptr != usize::default() {
                    info.gpu_output_ptr = usize::default();
                }
                if info.texture_rgba_ptr != usize::default() {
                    return;
                }
            }
            sessions_lock.remove(&display);
        } else {
            if let Some(info) = sessions_lock.get_mut(&display) {
                if info.gpu_output_ptr != usize::default() && info.gpu_output_ptr != ptr {
                    log::error!(
                        "gpu_output_ptr is not null and not equal to ptr, relace {} to {}",
                        info.gpu_output_ptr,
                        ptr
                    );
                }
                info.gpu_output_ptr = ptr as _;
                info.notify_render_type = None;
            } else {
                if ptr != usize::default() {
                    sessions_lock.insert(
                        display,
                        DisplaySessionInfo {
                            texture_rgba_ptr: usize::default(),
                            size: (0, 0),
                            gpu_output_ptr: ptr,
                            notify_render_type: None,
                        },
                    );
                }
            }
        }
    }

    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    pub fn on_rgba(&self, display: usize, rgba: &scrap::ImageRgb) -> bool {
        let mut write_lock = self.map_display_sessions.write().unwrap();
        let opt_info = if !self.is_support_multi_ui_session {
            write_lock.values_mut().next()
        } else {
            write_lock.get_mut(&display)
        };
        let Some(info) = opt_info else {
            return false;
        };
        if info.texture_rgba_ptr == usize::default() {
            return false;
        }

        if info.size.0 != rgba.w || info.size.1 != rgba.h {
            log::error!(
                "width/height mismatch: ({},{}) != ({},{})",
                info.size.0,
                info.size.1,
                rgba.w,
                rgba.h
            );
            // Peer info's handling is async and may be late than video frame's handling
            // Allow peer info not set, but not allow wrong width/height for correct local cursor position
            if info.size != (0, 0) {
                return false;
            }
        }
        if let Some(func) = &self.on_rgba_func {
            unsafe {
                func(
                    info.texture_rgba_ptr as _,
                    rgba.raw.as_ptr() as _,
                    rgba.raw.len() as _,
                    rgba.w as _,
                    rgba.h as _,
                    rgba.align() as _,
                )
            };
        }
        if info.notify_render_type != Some(RenderType::PixelBuffer) {
            info.notify_render_type = Some(RenderType::PixelBuffer);
            true
        } else {
            false
        }
    }

    #[cfg(feature = "vram")]
    pub fn on_texture(&self, display: usize, texture: *mut c_void) -> bool {
        let mut write_lock = self.map_display_sessions.write().unwrap();
        let opt_info = if !self.is_support_multi_ui_session {
            write_lock.values_mut().next()
        } else {
            write_lock.get_mut(&display)
        };
        let Some(info) = opt_info else {
            return false;
        };
        if info.gpu_output_ptr == usize::default() {
            return false;
        }
        if let Some(func) = &self.on_texture_func {
            unsafe { func(info.gpu_output_ptr as _, texture) };
        }
        if info.notify_render_type != Some(RenderType::Texture) {
            info.notify_render_type = Some(RenderType::Texture);
            true
        } else {
            false
        }
    }

    pub fn reset_all_display_render_type(&self) {
        let mut write_lock = self.map_display_sessions.write().unwrap();
        write_lock
            .values_mut()
            .map(|v| v.notify_render_type = None)
            .count();
    }
}

impl SessionHandler {
    pub fn on_waiting_for_image_dialog_show(&self) {
        self.renderer.reset_all_display_render_type();
        // rgba array render will notify every frame
    }
}

impl FlutterHandler {
    /// Push an event to all the event queues.
    /// An event is stored as json in the event queues.
    ///
    /// # Arguments
    ///
    /// * `name` - The name of the event.
    /// * `event` - Fields of the event content.
    pub fn push_event<V>(&self, name: &str, event: &[(&str, V)], excludes: &[&SessionID])
    where
        V: Sized + Serialize + Clone,
    {
        self.push_event_(name, event, &[], excludes);
    }

    pub fn push_event_to<V>(&self, name: &str, event: &[(&str, V)], include: &[&SessionID])
    where
        V: Sized + Serialize + Clone,
    {
        self.push_event_(name, event, include, &[]);
    }

    pub fn push_event_<V>(
        &self,
        name: &str,
        event: &[(&str, V)],
        includes: &[&SessionID],
        excludes: &[&SessionID],
    ) where
        V: Sized + Serialize + Clone,
    {
        let mut h: HashMap<&str, serde_json::Value> =
            event.iter().map(|(k, v)| (*k, json!(*v))).collect();
        debug_assert!(h.get("name").is_none());
        h.insert("name", json!(name));
        let out = serde_json::ser::to_string(&h).unwrap_or("".to_owned());
        for (sid, session) in self.session_handlers.read().unwrap().iter() {
            let mut push = false;
            if includes.is_empty() {
                if !excludes.contains(&sid) {
                    push = true;
                }
            } else {
                if includes.contains(&sid) {
                    push = true;
                }
            }
            if push {
                if let Some(stream) = &session.event_stream {
                    stream.add(EventToUI::Event(out.clone()));
                }
            }
        }
    }

    pub(crate) fn close_event_stream(&self, session_id: SessionID) {
        // to-do: Make sure the following logic is correct.
        // No need to remove the display handler, because it will be removed when the connection is closed.
        if let Some(session) = self.session_handlers.write().unwrap().get_mut(&session_id) {
            try_send_close_event(&session.event_stream);
        }
    }

    fn make_displays_msg(displays: &Vec<DisplayInfo>) -> String {
        let mut msg_vec = Vec::new();
        for ref d in displays.iter() {
            let mut h: HashMap<&str, i32> = Default::default();
            h.insert("x", d.x);
            h.insert("y", d.y);
            h.insert("width", d.width);
            h.insert("height", d.height);
            h.insert("cursor_embedded", if d.cursor_embedded { 1 } else { 0 });
            if let Some(original_resolution) = d.original_resolution.as_ref() {
                h.insert("original_width", original_resolution.width);
                h.insert("original_height", original_resolution.height);
            }
            // Don't convert scale (x 100) to i32 directly.
            // (d.scale * 100.0f64) as i32 may produces inaccuracies.
            //
            // Example: GNOME Wayland with Fractional Scaling enabled:
            // - Physical resolution: 2560x1600
            // - Logical resolution: 1074x1065
            // - Scale factor: 150%
            // Passing physical dimensions and scale factor prevents accurate logical resolution calculation
            // since 2560/1.5 = 1706.666... (rounded to 1706.67) and 1600/1.5 = 1066.666... (rounded to 1066.67)
            // h.insert("scale", (d.scale * 100.0f64) as i32);

            // Send scaled_width for accurate logical scale calculation.
            if d.scale > 0.0 {
                let scaled_width = (d.width as f64 / d.scale).round() as i32;
                h.insert("scaled_width", scaled_width);
            }
            msg_vec.push(h);
        }
        serde_json::ser::to_string(&msg_vec).unwrap_or("".to_owned())
    }

    #[cfg(feature = "plugin_framework")]
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    pub(crate) fn add_session_hook(&self, key: String, hook: SessionHook) -> bool {
        let mut hooks = self.hooks.write().unwrap();
        if hooks.contains_key(&key) {
            // Already has the hook with this key.
            return false;
        }
        let _ = hooks.insert(key, hook);
        true
    }

    #[cfg(feature = "plugin_framework")]
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    pub(crate) fn remove_session_hook(&self, key: &String) -> bool {
        let mut hooks = self.hooks.write().unwrap();
        if !hooks.contains_key(key) {
            // The hook with this key does not found.
            return false;
        }
        let _ = hooks.remove(key);
        true
    }

    pub fn update_use_texture_render(&self) {
        self.use_texture_render
            .store(crate::ui_interface::use_texture_render(), Ordering::Relaxed);
        self.display_rgbas.write().unwrap().clear();
    }
}

// lobishell-android (host cursor shape): `set_cursor_data`/`set_cursor_id` below only ever
// `push_event(...)`, which is a no-op for a headless session — `session_start_headless`
// deliberately never sets `event_stream` (no Dart isolate to push to), so the peer's cursor
// shapes were decoded and then dropped on the floor. These statics give our JNI shim a
// poll-based way to observe them, exactly like `scrap::android::ffi`'s clipboard buffers
// (`set_remote_clipboard_text`/`take_remote_clipboard_text`) do for clipboard text.
//
// GLOBAL rather than per-session (same tradeoff the clipboard buffers already make): the shim
// only ever drives one active session at a time, and threading a SessionID through here would
// mean plumbing it into `InvokeUiSession`, whose methods take `&self` on a handler that doesn't
// know its own session id. `take_headless_cursor` therefore ignores which session asked.
//
// The cache is required, not an optimization: the peer sends a full `CursorData` shape ONCE per
// distinct cursor and thereafter refers back to it by id via `CursorId`. Without keeping the
// shapes, every `CursorId` would leave us with nothing to draw.
struct HeadlessCursorShape {
    width: i32,
    height: i32,
    hotx: i32,
    hoty: i32,
    /// Decompressed RGBA, 4 bytes/pixel, `width * height * 4` long.
    colors: Vec<u8>,
}

struct HeadlessCursorState {
    shapes: HashMap<u64, HeadlessCursorShape>,
    /// Insertion order, used to evict the oldest shape once `MAX_HEADLESS_CURSOR_SHAPES` is hit
    /// so a long session can't grow `shapes` without bound. Hosts cycle through a modest set of
    /// cursors, so eviction should be rare in practice; if an evicted id is later referenced by a
    /// `CursorId`, we report nothing rather than drawing garbage.
    order: std::collections::VecDeque<u64>,
    /// Id of the cursor the peer says is currently shown, if its shape is known.
    current: Option<u64>,
    /// Set whenever `current` changes (or its shape is redefined); cleared by
    /// `take_headless_cursor`, giving the same clear-on-read semantics as `getFrame`'s rgba
    /// buffer — the caller polls on its video loop and only rebuilds its Bitmap on a change.
    dirty: bool,
}

const MAX_HEADLESS_CURSOR_SHAPES: usize = 32;

lazy_static::lazy_static! {
    static ref HEADLESS_CURSOR: std::sync::Mutex<HeadlessCursorState> =
        std::sync::Mutex::new(HeadlessCursorState {
            shapes: HashMap::new(),
            order: std::collections::VecDeque::new(),
            current: None,
            dirty: false,
        });
}

/// lobishell-android: caches a decompressed cursor shape by id and marks it current. Called from
/// `set_cursor_data` (see `HeadlessCursorShape`'s doc above for why).
fn cache_headless_cursor(cd: &CursorData, colors: Vec<u8>) {
    let mut state = HEADLESS_CURSOR.lock().unwrap();
    let is_new = state
        .shapes
        .insert(
            cd.id,
            HeadlessCursorShape {
                width: cd.width,
                height: cd.height,
                hotx: cd.hotx,
                hoty: cd.hoty,
                colors,
            },
        )
        .is_none();
    if is_new {
        state.order.push_back(cd.id);
        while state.order.len() > MAX_HEADLESS_CURSOR_SHAPES {
            match state.order.pop_front() {
                // Never evict the shape we're currently showing.
                Some(old) if Some(old) == state.current => {
                    state.order.push_back(old);
                    break;
                }
                Some(old) => {
                    state.shapes.remove(&old);
                }
                None => break,
            }
        }
    }
    state.current = Some(cd.id);
    state.dirty = true;
}

/// lobishell-android: marks an already-cached shape current. Called from `set_cursor_id`. An id
/// we've never seen a `CursorData` for (or whose shape was evicted) leaves `current` untouched
/// rather than blanking the cursor.
fn select_headless_cursor(id: u64) {
    let mut state = HEADLESS_CURSOR.lock().unwrap();
    if !state.shapes.contains_key(&id) {
        return;
    }
    if state.current != Some(id) {
        state.current = Some(id);
        state.dirty = true;
    }
}

/// lobishell-android: takes the current cursor if it changed since the last call, packed for the
/// JNI shim as 4 little-endian i32s (`width`, `height`, `hotx`, `hoty`) followed by
/// `width * height * 4` bytes of RGBA pixel data. Returns None when unchanged (or when nothing is
/// known yet), i.e. clear-on-read polling — same contract as `take_remote_clipboard_text`.
pub fn take_headless_cursor() -> Option<Vec<u8>> {
    let mut state = HEADLESS_CURSOR.lock().unwrap();
    if !state.dirty {
        return None;
    }
    let id = state.current?;
    let out = {
        let shape = state.shapes.get(&id)?;
        let mut out = Vec::with_capacity(16 + shape.colors.len());
        out.extend_from_slice(&shape.width.to_le_bytes());
        out.extend_from_slice(&shape.height.to_le_bytes());
        out.extend_from_slice(&shape.hotx.to_le_bytes());
        out.extend_from_slice(&shape.hoty.to_le_bytes());
        out.extend_from_slice(&shape.colors);
        out
    };
    state.dirty = false;
    Some(out)
}

// ---------------------------------------------------------------------------------------------
// lobishell-android (audio playback, plans/soft-frolicking-thimble.md "Audio" round):
// `AudioHandler::handle_frame` (client.rs) decodes each incoming `AudioFrame` into f32 PCM on ITS
// OWN dedicated thread (`client::start_audio_thread`) — a thread the headless setup gets for
// free, since nothing in that code path goes through `InvokeUiSession`/`push_event` at all (unlike
// video/cursor/clipboard, which needed the poll-cache workaround specifically because their
// natural path IS push_event). What's still missing headless is a way to get the decoded PCM (and
// the negotiated sample rate/channel count, which can change mid-session) OUT to the JNI shim —
// this cache provides that. Global rather than per-session, same tradeoff as the cursor/clipboard
// caches: only one active control session drives audio at a time in this app.
// ---------------------------------------------------------------------------------------------
struct HeadlessAudioState {
    /// Set whenever the negotiated format changes; cleared by `take_headless_audio_format`.
    format: Option<(u32, u16)>,
    /// Flat interleaved PCM sample queue — matches magnum_opus/opus-rs's own decode_float output
    /// shape directly, no repacking needed on the push side.
    pcm: std::collections::VecDeque<f32>,
}

// ~2s of 48kHz stereo audio: generous headroom for a Kotlin poll thread that's briefly stalled
// (e.g. a GC pause), small enough that a genuinely stuck consumer can't grow this unbounded.
const MAX_HEADLESS_AUDIO_SAMPLES: usize = 48_000 * 2 * 2;

lazy_static::lazy_static! {
    static ref HEADLESS_AUDIO: std::sync::Mutex<HeadlessAudioState> =
        std::sync::Mutex::new(HeadlessAudioState { format: None, pcm: std::collections::VecDeque::new() });
}

/// lobishell-android: called from `AudioHandler::handle_format` (client.rs, Android branch) when
/// the peer's audio format is (re)negotiated.
pub fn push_headless_audio_format(sample_rate: u32, channels: u16) {
    HEADLESS_AUDIO.lock().unwrap().format = Some((sample_rate, channels));
}

/// lobishell-android: called from `AudioHandler::handle_frame` (client.rs, Android branch) with
/// each frame's decoded PCM. Drops the OLDEST samples on overflow rather than the newest — smooth
/// continuous playback needs continuity more than completeness, the same tradeoff any live audio
/// ring buffer makes under backpressure.
pub fn push_headless_audio_pcm(samples: &[f32]) {
    let mut state = HEADLESS_AUDIO.lock().unwrap();
    state.pcm.extend(samples.iter().copied());
    while state.pcm.len() > MAX_HEADLESS_AUDIO_SAMPLES {
        state.pcm.pop_front();
    }
}

/// lobishell-android: clear-on-change poll for the negotiated format — returns `(sample_rate,
/// channels)` once right after it's (re)set, then `None` until it changes again. The JNI shim
/// (re)builds its `AudioTrack` whenever this returns non-null.
pub fn take_headless_audio_format() -> Option<(u32, u16)> {
    HEADLESS_AUDIO.lock().unwrap().format.take()
}

/// lobishell-android: drains up to `max_samples` interleaved PCM samples (FIFO order), or an
/// empty Vec if none are pending yet.
pub fn take_headless_audio_pcm(max_samples: usize) -> Vec<f32> {
    let mut state = HEADLESS_AUDIO.lock().unwrap();
    let n = max_samples.min(state.pcm.len());
    state.pcm.drain(0..n).collect()
}

// ---------------------------------------------------------------------------------------------
// lobishell-android (auto quality / connection stats): `update_quality_status` already fires
// periodically with everything an adaptive-quality heuristic or a latency/bandwidth readout
// needs — `delay`/`target_bitrate` from the server's own TestDelay ping-pong (roughly every few
// seconds, see ui_session_interface.rs's handle_test_delay), `speed`/`fps`/`codec_format`/`chroma`
// from a separate ~1s stats tick (client/io_loop.rs). Nothing new needs to be requested from the
// peer — this cache just gets the data OUT of the no-op push_event path, same as cursor/clipboard/
// audio before it. LATEST-VALUE overwrite (not a queue — only the current status matters), with
// per-field MERGE rather than wholesale replacement: the two update sites above each only ever
// populate a subset of QualityStatus's fields (`..Default::default()` for the rest), so a
// `delay`-only update must not blank out the `speed`/`fps` fields the OTHER call site set most
// recently, and vice versa.
#[derive(Default, Clone)]
struct HeadlessQualityStatus {
    speed: Option<String>,
    fps: Option<i32>,
    delay: Option<i32>,
    target_bitrate: Option<i32>,
    codec_format: Option<String>,
}

lazy_static::lazy_static! {
    static ref HEADLESS_QUALITY: std::sync::Mutex<HeadlessQualityStatus> =
        std::sync::Mutex::new(HeadlessQualityStatus::default());
}

fn push_headless_quality_status(status: &QualityStatus) {
    let mut state = HEADLESS_QUALITY.lock().unwrap();
    if status.speed.is_some() {
        state.speed = status.speed.clone();
    }
    // Only one display in this headless client (see session_get_display_size's own single-display
    // framing) — take whichever fps value is present rather than exposing the whole per-display map.
    if let Some(fps) = status.fps.values().next() {
        state.fps = Some(*fps);
    }
    if status.delay.is_some() {
        state.delay = status.delay;
    }
    if status.target_bitrate.is_some() {
        state.target_bitrate = status.target_bitrate;
    }
    if status.codec_format.is_some() {
        state.codec_format = status.codec_format.map(|f| f.to_string());
    }
}

/// lobishell-android: the latest known connection stats as a JSON object, or `"{}"` if nothing has
/// arrived yet. Any field not yet known is simply omitted (not written as `null`), so the caller
/// can treat a missing key exactly like "unknown" without a separate null-check. Layout:
/// `{"speed":"1.2MB/s","fps":30,"delay":45,"target_bitrate":2000,"codec_format":"VP9"}`.
pub fn take_headless_quality_status() -> String {
    let state = HEADLESS_QUALITY.lock().unwrap().clone();
    let mut obj = serde_json::Map::new();
    if let Some(speed) = state.speed {
        obj.insert("speed".to_string(), serde_json::Value::String(speed));
    }
    if let Some(fps) = state.fps {
        obj.insert("fps".to_string(), serde_json::Value::from(fps));
    }
    if let Some(delay) = state.delay {
        obj.insert("delay".to_string(), serde_json::Value::from(delay));
    }
    if let Some(target_bitrate) = state.target_bitrate {
        obj.insert("target_bitrate".to_string(), serde_json::Value::from(target_bitrate));
    }
    if let Some(codec_format) = state.codec_format {
        obj.insert("codec_format".to_string(), serde_json::Value::String(codec_format));
    }
    serde_json::Value::Object(obj).to_string()
}

// lobishell-android (file transfer): the file-transfer InvokeUiSession callbacks below
// (`update_folder_files`, `job_progress`, `job_done`, `job_error`, `override_file_confirm`) only
// ever `push_event(...)`, which is a no-op for a headless session (`session_start_headless` never
// sets `event_stream`). These queues give our JNI shim a poll-based way to drain those events as
// JSON strings — same idea as the cursor/clipboard poll caches, but keyed per `SessionID` because
// a user can plausibly run several independent file-transfer sessions (to different peers) at
// once, and the file-transfer session is a SEPARATE session from the control/video one anyway.
//
// The FlutterHandler methods don't receive a `SessionID`, but a headless file-transfer session's
// handler has exactly one entry in `session_handlers` (its own ui session), so we recover it via
// `headless_session_id()` below.
#[derive(Default)]
struct HeadlessFtQueues {
    /// FIFO of directory-listing results, each a JSON string (see `ft_push_dir_listing`).
    dir_listings: std::collections::VecDeque<String>,
    /// FIFO of job status events (progress / done / error), each a JSON string.
    job_events: std::collections::VecDeque<String>,
    /// Single pending overwrite/resume prompt (clear-on-read). The job blocks until it's answered
    /// via `session_set_confirm_override_file`, so at most one is outstanding at a time.
    override_confirm: Option<String>,
}

// Bounds so a session whose consumer stops polling can't grow these without limit. Directory
// listings and job events are both low-rate; these caps are generous and only drop the oldest.
const MAX_FT_DIR_LISTINGS: usize = 64;
const MAX_FT_JOB_EVENTS: usize = 512;

lazy_static::lazy_static! {
    static ref HEADLESS_FT: std::sync::Mutex<HashMap<SessionID, HeadlessFtQueues>> =
        std::sync::Mutex::new(HashMap::new());
}

fn ft_push_dir_listing(session_id: SessionID, json: String) {
    let mut map = HEADLESS_FT.lock().unwrap();
    let q = map.entry(session_id).or_default();
    q.dir_listings.push_back(json);
    while q.dir_listings.len() > MAX_FT_DIR_LISTINGS {
        q.dir_listings.pop_front();
    }
}

fn ft_push_job_event(session_id: SessionID, json: String) {
    let mut map = HEADLESS_FT.lock().unwrap();
    let q = map.entry(session_id).or_default();
    q.job_events.push_back(json);
    while q.job_events.len() > MAX_FT_JOB_EVENTS {
        q.job_events.pop_front();
    }
}

fn ft_set_override_confirm(session_id: SessionID, json: String) {
    let mut map = HEADLESS_FT.lock().unwrap();
    map.entry(session_id).or_default().override_confirm = Some(json);
}

/// lobishell-android: drain the oldest pending directory-listing JSON for `session_id`, or None if
/// the queue is empty (FIFO clear-on-read, one entry per call).
pub fn ft_take_dir_listing(session_id: &SessionID) -> Option<String> {
    HEADLESS_FT
        .lock()
        .unwrap()
        .get_mut(session_id)?
        .dir_listings
        .pop_front()
}

/// lobishell-android: drain the oldest pending job-status JSON for `session_id`, or None if empty.
pub fn ft_take_job_event(session_id: &SessionID) -> Option<String> {
    HEADLESS_FT
        .lock()
        .unwrap()
        .get_mut(session_id)?
        .job_events
        .pop_front()
}

/// lobishell-android: take the pending overwrite-confirm prompt for `session_id` (clear-on-read),
/// or None if none is pending.
pub fn ft_take_override_confirm(session_id: &SessionID) -> Option<String> {
    HEADLESS_FT
        .lock()
        .unwrap()
        .get_mut(session_id)?
        .override_confirm
        .take()
}

fn ft_entries_to_json(entries: &Vec<FileEntry>) -> Vec<serde_json::Value> {
    entries
        .iter()
        .map(|e| {
            json!({
                // FileType enum int value: 0=Dir, 2=DirLink, 3=DirDrive, 4=File, 5=FileLink.
                "entry_type": e.entry_type.value(),
                "name": e.name,
                "is_hidden": e.is_hidden,
                "size": e.size,
                "modified_time": e.modified_time,
            })
        })
        .collect()
}

impl FlutterHandler {
    /// lobishell-android: the SessionID of this (headless) handler's single ui session. A
    /// headless file-transfer session has exactly one entry in `session_handlers`, so returning
    /// the first key identifies it. Returns None only if called before the handler is registered.
    fn headless_session_id(&self) -> Option<SessionID> {
        self.session_handlers.read().unwrap().keys().next().copied()
    }
}

impl InvokeUiSession for FlutterHandler {
    fn set_cursor_data(&self, cd: CursorData) {
        let mut colors = hbb_common::compress::decompress(&cd.colors);
        // lobishell-android: same workaround src/ui/remote.rs's SciterHandler::set_cursor_data
        // applies ("somehow all 0 images shows black rect") — an all-zero (fully transparent)
        // buffer renders as an opaque black rectangle, so nudge one alpha byte.
        if colors.len() > 3 && colors.iter().filter(|x| **x != 0).next().is_none() {
            log::info!("Fix transparent");
            colors[3] = 1;
        }
        // lobishell-android: cache the shape for our poll-based JNI getter before the (headless
        // no-op) push_event below, which is left intact so a future non-headless use still works.
        cache_headless_cursor(&cd, colors.clone());
        self.push_event(
            "cursor_data",
            &[
                ("id", &cd.id.to_string()),
                ("hotx", &cd.hotx.to_string()),
                ("hoty", &cd.hoty.to_string()),
                ("width", &cd.width.to_string()),
                ("height", &cd.height.to_string()),
                (
                    "colors",
                    &serde_json::ser::to_string(&colors).unwrap_or("".to_owned()),
                ),
            ],
            &[],
        );
    }

    fn set_cursor_id(&self, id: String) {
        // lobishell-android: the peer refers back to an already-sent shape by id — select it in
        // our cache so the headless getter reports the right cursor (see cache_headless_cursor).
        if let Ok(id) = id.parse::<u64>() {
            select_headless_cursor(id);
        }
        self.push_event("cursor_id", &[("id", &id.to_string())], &[]);
    }

    fn set_cursor_position(&self, cp: CursorPosition) {
        self.push_event(
            "cursor_position",
            &[("x", &cp.x.to_string()), ("y", &cp.y.to_string())],
            &[],
        );
    }

    /// unused in flutter, use switch_display or set_peer_info
    fn set_display(&self, _x: i32, _y: i32, _w: i32, _h: i32, _cursor_embedded: bool, _scale: f64) {}

    fn update_privacy_mode(&self) {
        self.push_event::<&str>("update_privacy_mode", &[], &[]);
    }

    fn set_permission(&self, name: &str, value: bool) {
        self.push_event("permission", &[(name, &value.to_string())], &[]);
    }

    // unused in flutter
    fn close_success(&self) {}

    fn update_quality_status(&self, status: QualityStatus) {
        // lobishell-android (auto quality / connection stats): also cache the latest status for
        // the headless poll consumer before the (no-op) push_event below — see
        // `push_headless_quality_status`'s doc. RustDesk already sends everything an auto-quality
        // heuristic needs for free: `delay`/`target_bitrate` arrive periodically via the server's
        // own TestDelay ping-pong (see ui_session_interface.rs's handle_test_delay), `speed`/`fps`
        // via a separate ~1s stats tick (client/io_loop.rs) — nothing new to request from the peer.
        push_headless_quality_status(&status);
        const NULL: String = String::new();
        self.push_event(
            "update_quality_status",
            &[
                ("speed", &status.speed.map_or(NULL, |it| it)),
                (
                    "fps",
                    &serde_json::ser::to_string(&status.fps).unwrap_or(NULL.to_owned()),
                ),
                ("delay", &status.delay.map_or(NULL, |it| it.to_string())),
                (
                    "target_bitrate",
                    &status.target_bitrate.map_or(NULL, |it| it.to_string()),
                ),
                (
                    "codec_format",
                    &status.codec_format.map_or(NULL, |it| it.to_string()),
                ),
                ("chroma", &status.chroma.map_or(NULL, |it| it.to_string())),
            ],
            &[],
        );
    }

    fn set_connection_type(&self, is_secured: bool, direct: bool, stream_type: &str) {
        self.push_event(
            "connection_ready",
            &[
                ("secure", &is_secured.to_string()),
                ("direct", &direct.to_string()),
                ("stream_type", &stream_type.to_string()),
            ],
            &[],
        );
    }

    fn set_fingerprint(&self, fingerprint: String) {
        self.push_event("fingerprint", &[("fingerprint", &fingerprint)], &[]);
    }

    fn job_error(&self, id: i32, err: String, file_num: i32) {
        // lobishell-android: also queue for the headless poll cache before the (no-op) push_event.
        if let Some(session_id) = self.headless_session_id() {
            ft_push_job_event(
                session_id,
                json!({
                    "type": "error",
                    "id": id,
                    "file_num": file_num,
                    "err": err,
                })
                .to_string(),
            );
        }
        self.push_event(
            "job_error",
            &[
                ("id", &id.to_string()),
                ("err", &err),
                ("file_num", &file_num.to_string()),
            ],
            &[],
        );
    }

    fn job_done(&self, id: i32, file_num: i32) {
        // lobishell-android: also queue for the headless poll cache before the (no-op) push_event.
        if let Some(session_id) = self.headless_session_id() {
            ft_push_job_event(
                session_id,
                json!({
                    "type": "done",
                    "id": id,
                    "file_num": file_num,
                })
                .to_string(),
            );
        }
        self.push_event(
            "job_done",
            &[("id", &id.to_string()), ("file_num", &file_num.to_string())],
            &[],
        );
    }

    // unused in flutter
    fn clear_all_jobs(&self) {}

    fn load_last_job(&self, _cnt: i32, job_json: &str, _auto_start: bool) {
        self.push_event("load_last_job", &[("value", job_json)], &[]);
    }

    fn update_folder_files(
        &self,
        id: i32,
        entries: &Vec<FileEntry>,
        path: String,
        #[allow(unused_variables)] is_local: bool,
        only_count: bool,
    ) {
        // lobishell-android: also queue the listing for the headless poll cache before the (no-op)
        // push_event below. Covers both a real remote listing (`is_local == false`) and the
        // local-preview count that a read job emits (`is_local == true`, `only_count == true`).
        if let Some(session_id) = self.headless_session_id() {
            ft_push_dir_listing(
                session_id,
                json!({
                    "id": id,
                    "path": path,
                    "is_local": is_local,
                    "only_count": only_count,
                    "entries": ft_entries_to_json(entries),
                })
                .to_string(),
            );
        }
        // TODO opt
        if only_count {
            self.push_event(
                "update_folder_files",
                &[("info", &make_fd_flutter(id, entries, only_count))],
                &[],
            );
        } else {
            self.push_event(
                "file_dir",
                &[
                    ("is_local", "false"),
                    ("value", &crate::common::make_fd_to_json(id, path, entries)),
                ],
                &[],
            );
        }
    }

    fn update_empty_dirs(&self, res: ReadEmptyDirsResponse) {
        self.push_event(
            "empty_dirs",
            &[
                ("is_local", "false"),
                (
                    "value",
                    &crate::common::make_empty_dirs_response_to_json(&res),
                ),
            ],
            &[],
        );
    }

    // unused in flutter
    fn update_transfer_list(&self) {}

    // unused in flutter // TEST flutter
    fn confirm_delete_files(&self, _id: i32, _i: i32, _name: String) {}

    fn override_file_confirm(
        &self,
        id: i32,
        file_num: i32,
        to: String,
        is_upload: bool,
        is_identical: bool,
    ) {
        // lobishell-android: stash the prompt for the headless poll cache (clear-on-read) before
        // the (no-op) push_event. The job blocks until answered via
        // `session_set_confirm_override_file`, so a single pending slot is correct.
        if let Some(session_id) = self.headless_session_id() {
            ft_set_override_confirm(
                session_id,
                json!({
                    "id": id,
                    "file_num": file_num,
                    "to": to,
                    "is_upload": is_upload,
                    "is_identical": is_identical,
                })
                .to_string(),
            );
        }
        self.push_event(
            "override_file_confirm",
            &[
                ("id", &id.to_string()),
                ("file_num", &file_num.to_string()),
                ("read_path", &to),
                ("is_upload", &is_upload.to_string()),
                ("is_identical", &is_identical.to_string()),
            ],
            &[],
        );
    }

    fn job_progress(&self, id: i32, file_num: i32, speed: f64, finished_size: f64) {
        // lobishell-android: also queue for the headless poll cache before the (no-op) push_event.
        if let Some(session_id) = self.headless_session_id() {
            ft_push_job_event(
                session_id,
                json!({
                    "type": "progress",
                    "id": id,
                    "file_num": file_num,
                    "speed": speed,
                    "finished_size": finished_size,
                })
                .to_string(),
            );
        }
        self.push_event(
            "job_progress",
            &[
                ("id", &id.to_string()),
                ("file_num", &file_num.to_string()),
                ("speed", &speed.to_string()),
                ("finished_size", &finished_size.to_string()),
            ],
            &[],
        );
    }

    // unused in flutter
    fn adapt_size(&self) {}

    #[inline]
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    fn on_rgba(&self, display: usize, rgba: &mut scrap::ImageRgb) {
        let use_texture_render = self.use_texture_render.load(Ordering::Relaxed);
        self.on_rgba_flutter_texture_render(use_texture_render, display, rgba);
        if !use_texture_render {
            self.on_rgba_soft_render(display, rgba);
        }
    }

    #[inline]
    #[cfg(any(target_os = "android", target_os = "ios"))]
    fn on_rgba(&self, display: usize, rgba: &mut scrap::ImageRgb) {
        self.on_rgba_soft_render(display, rgba);
    }

    #[inline]
    #[cfg(feature = "vram")]
    fn on_texture(&self, display: usize, texture: *mut c_void) {
        if !self.use_texture_render.load(Ordering::Relaxed) {
            return;
        }
        for (_, session) in self.session_handlers.read().unwrap().iter() {
            if session.renderer.on_texture(display, texture) {
                if let Some(stream) = &session.event_stream {
                    stream.add(EventToUI::Texture(display, true));
                }
            }
        }
    }

    fn set_peer_info(&self, pi: &PeerInfo) {
        let displays = Self::make_displays_msg(&pi.displays);
        let mut features: HashMap<&str, bool> = Default::default();
        for ref f in pi.features.iter() {
            features.insert("privacy_mode", f.privacy_mode);
        }
        // compatible with 1.1.9
        if get_version_number(&pi.version) < get_version_number("1.2.0") {
            features.insert("privacy_mode", false);
        }
        let features = serde_json::ser::to_string(&features).unwrap_or("".to_owned());
        let resolutions = serialize_resolutions(&pi.resolutions.resolutions);
        *self.peer_info.write().unwrap() = pi.clone();
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        let is_support_multi_ui_session = crate::common::is_support_multi_ui_session(&pi.version);
        #[cfg(any(target_os = "android", target_os = "ios"))]
        let is_support_multi_ui_session = false;
        self.session_handlers
            .write()
            .unwrap()
            .values_mut()
            .for_each(|h| {
                h.renderer.is_support_multi_ui_session = is_support_multi_ui_session;
            });
        self.push_event(
            "peer_info",
            &[
                ("username", &pi.username),
                ("hostname", &pi.hostname),
                ("platform", &pi.platform),
                ("sas_enabled", &pi.sas_enabled.to_string()),
                ("displays", &displays),
                ("version", &pi.version),
                ("features", &features),
                ("current_display", &pi.current_display.to_string()),
                ("resolutions", &resolutions),
                ("platform_additions", &pi.platform_additions),
            ],
            &[],
        );
    }

    fn set_displays(&self, displays: &Vec<DisplayInfo>) {
        self.peer_info.write().unwrap().displays = displays.clone();
        self.push_event(
            "sync_peer_info",
            &[("displays", &Self::make_displays_msg(displays))],
            &[],
        );
    }

    fn set_platform_additions(&self, data: &str) {
        self.push_event(
            "sync_platform_additions",
            &[("platform_additions", &data)],
            &[],
        )
    }

    fn set_multiple_windows_session(&self, sessions: Vec<WindowsSession>) {
        let mut msg_vec = Vec::new();
        let mut sessions = sessions;
        for d in sessions.drain(..) {
            let mut h: HashMap<&str, String> = Default::default();
            h.insert("sid", d.sid.to_string());
            h.insert("name", d.name);
            msg_vec.push(h);
        }
        self.push_event(
            "set_multiple_windows_session",
            &[(
                "windows_sessions",
                &serde_json::ser::to_string(&msg_vec).unwrap_or("".to_owned()),
            )],
            &[],
        );
    }

    fn is_multi_ui_session(&self) -> bool {
        self.session_handlers.read().unwrap().len() > 1
    }

    fn set_current_display(&self, disp_idx: i32) {
        if self.is_multi_ui_session() {
            return;
        }
        self.push_event(
            "follow_current_display",
            &[("display_idx", &disp_idx.to_string())],
            &[],
        );
    }

    fn on_connected(&self, _conn_type: ConnType) {}

    fn msgbox(&self, msgtype: &str, title: &str, text: &str, link: &str, retry: bool) {
        let has_retry = if retry { "true" } else { "" };
        self.push_event(
            "msgbox",
            &[
                ("type", msgtype),
                ("title", title),
                ("text", text),
                ("link", link),
                ("hasRetry", has_retry),
            ],
            &[],
        );
    }

    fn cancel_msgbox(&self, tag: &str) {
        self.push_event("cancel_msgbox", &[("tag", tag)], &[]);
    }

    fn new_message(&self, msg: String) {
        self.push_event("chat_client_mode", &[("text", &msg)], &[]);
    }

    fn switch_display(&self, display: &SwitchDisplay) {
        let resolutions = serialize_resolutions(&display.resolutions.resolutions);
        self.push_event(
            "switch_display",
            &[
                ("display", &display.display.to_string()),
                ("x", &display.x.to_string()),
                ("y", &display.y.to_string()),
                ("width", &display.width.to_string()),
                ("height", &display.height.to_string()),
                (
                    "cursor_embedded",
                    &{
                        if display.cursor_embedded {
                            1
                        } else {
                            0
                        }
                    }
                    .to_string(),
                ),
                ("resolutions", &resolutions),
                (
                    "original_width",
                    &display.original_resolution.width.to_string(),
                ),
                (
                    "original_height",
                    &display.original_resolution.height.to_string(),
                ),
            ],
            &[],
        );
    }

    fn update_block_input_state(&self, on: bool) {
        self.push_event(
            "update_block_input_state",
            &[("input_state", if on { "on" } else { "off" })],
            &[],
        );
    }

    #[cfg(any(target_os = "android", target_os = "ios"))]
    fn clipboard(&self, content: String) {
        self.push_event("clipboard", &[("content", &content)], &[]);
    }

    fn switch_back(&self, peer_id: &str) {
        self.push_event("switch_back", &[("peer_id", peer_id)], &[]);
    }

    fn portable_service_running(&self, running: bool) {
        self.push_event(
            "portable_service_running",
            &[("running", running.to_string().as_str())],
            &[],
        );
    }

    fn on_voice_call_started(&self) {
        self.push_event::<&str>("on_voice_call_started", &[], &[]);
    }

    fn on_voice_call_closed(&self, reason: &str) {
        let _res = self.push_event("on_voice_call_closed", &[("reason", reason)], &[]);
    }

    fn on_voice_call_waiting(&self) {
        self.push_event::<&str>("on_voice_call_waiting", &[], &[]);
    }

    fn on_voice_call_incoming(&self) {
        self.push_event::<&str>("on_voice_call_incoming", &[], &[]);
    }

    #[inline]
    fn get_rgba(&self, _display: usize) -> *const u8 {
        if let Some(rgba_data) = self.display_rgbas.read().unwrap().get(&_display) {
            if rgba_data.valid {
                return rgba_data.data.as_ptr();
            }
        }
        std::ptr::null_mut()
    }

    #[inline]
    fn next_rgba(&self, _display: usize) {
        if let Some(rgba_data) = self.display_rgbas.write().unwrap().get_mut(&_display) {
            rgba_data.valid = false;
        }
    }

    fn update_record_status(&self, start: bool) {
        self.push_event("record_status", &[("start", &start.to_string())], &[]);
    }

    fn printer_request(&self, id: i32, path: String) {
        self.push_event(
            "printer_request",
            &[("id", json!(id)), ("path", json!(path))],
            &[],
        );
    }

    fn handle_screenshot_resp(&self, sid: String, msg: String) {
        match SessionID::from_str(&sid) {
            Ok(sid) => self.push_event_to("screenshot", &[("msg", json!(msg))], &[&sid]),
            Err(e) => {
                // Unreachable!
                log::error!("Failed to parse sid \"{}\", {}", sid, e);
            }
        }
    }

    fn handle_terminal_response(&self, response: TerminalResponse) {
        use hbb_common::message_proto::terminal_response::Union;

        match response.union {
            Some(Union::Opened(opened)) => {
                let mut event_data: Vec<(&str, serde_json::Value)> = vec![
                    ("type", json!("opened")),
                    ("terminal_id", json!(opened.terminal_id)),
                    ("success", json!(opened.success)),
                    ("message", json!(&opened.message)),
                    ("pid", json!(opened.pid)),
                    ("service_id", json!(&opened.service_id)),
                    (
                        "replay_terminal_output",
                        json!(opened.replay_terminal_output),
                    ),
                ];
                if !opened.persistent_sessions.is_empty() {
                    event_data.push(("persistent_sessions", json!(opened.persistent_sessions)));
                }
                self.push_event_("terminal_response", &event_data, &[], &[]);
            }
            Some(Union::Data(data)) => {
                // Decompress data if needed
                let output_data = if data.compressed {
                    hbb_common::compress::decompress(&data.data)
                } else {
                    data.data.to_vec()
                };

                let encoded = crate::encode64(&output_data);
                let event_data: Vec<(&str, serde_json::Value)> = vec![
                    ("type", json!("data")),
                    ("terminal_id", json!(data.terminal_id)),
                    ("data", json!(&encoded)),
                ];
                self.push_event_("terminal_response", &event_data, &[], &[]);
            }
            Some(Union::Closed(closed)) => {
                let event_data: Vec<(&str, serde_json::Value)> = vec![
                    ("type", json!("closed")),
                    ("terminal_id", json!(closed.terminal_id)),
                    ("exit_code", json!(closed.exit_code)),
                ];
                self.push_event_("terminal_response", &event_data, &[], &[]);
            }
            Some(Union::Error(error)) => {
                let event_data: Vec<(&str, serde_json::Value)> = vec![
                    ("type", json!("error")),
                    ("terminal_id", json!(error.terminal_id)),
                    ("message", json!(&error.message)),
                ];
                self.push_event_("terminal_response", &event_data, &[], &[]);
            }
            None => {}
            Some(_) => {
                log::warn!("Unhandled terminal response type");
            }
        }
    }
}

impl FlutterHandler {
    #[inline]
    fn on_rgba_soft_render(&self, display: usize, rgba: &mut scrap::ImageRgb) {
        // Give a chance for plugins or etc to hook a rgba data.
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        for (key, hook) in self.hooks.read().unwrap().iter() {
            match hook {
                SessionHook::OnSessionRgba(cb) => {
                    cb(key.to_owned(), rgba);
                }
            }
        }
        // If the current rgba is not fetched by flutter, i.e., is valid.
        // We give up sending a new event to flutter.
        let mut rgba_write_lock = self.display_rgbas.write().unwrap();
        if let Some(rgba_data) = rgba_write_lock.get_mut(&display) {
            if rgba_data.valid {
                return;
            } else {
                rgba_data.valid = true;
            }
            // Return the rgba buffer to the video handler for reusing allocated rgba buffer.
            std::mem::swap::<Vec<u8>>(&mut rgba.raw, &mut rgba_data.data);
        } else {
            let mut rgba_data = RgbaData::default();
            std::mem::swap::<Vec<u8>>(&mut rgba.raw, &mut rgba_data.data);
            rgba_data.valid = true;
            rgba_write_lock.insert(display, rgba_data);
        }
        drop(rgba_write_lock);

        let mut is_sent = false;
        let is_multi_sessions = self.is_multi_ui_session();
        for h in self.session_handlers.read().unwrap().values() {
            // The soft renderer does not support multi-displays session for now.
            if h.displays.len() > 1 {
                continue;
            }
            // If there're multiple ui sessions, we only notify the ui session that has the display.
            if is_multi_sessions {
                if !h.displays.contains(&display) {
                    continue;
                }
            }
            if let Some(stream) = &h.event_stream {
                stream.add(EventToUI::Rgba(display));
                is_sent = true;
            }
        }
        // We need `is_sent` here. Because we use texture render for multi-displays session.
        //
        // Eg. We have two windows, one is display 1, the other is displays 0&1.
        // When image of display 0 is received, we will not send the event.
        //
        // 1. "display 1" will not send the event.
        // 2. "displays 0&1" will not send the event. Because it uses texutre render for now.
        //
        // M3 (plans/soft-frolicking-thimble.md): this `!is_sent` branch assumed the ONLY way a
        // frame ever gets consumed is via a pushed `EventToUI::Rgba` event reaching a live
        // `event_stream` — so if nothing was notified, it eagerly discards the frame (valid=false)
        // rather than leave a stale one sitting around forever. `session_start_headless` (see its
        // own doc) intentionally NEVER sets `event_stream` (no Dart isolate to push to) — so
        // `is_sent` is unconditionally false for every single frame, and this reset the buffer
        // back to invalid within the SAME synchronous call that had just marked it valid a few
        // lines above. Our own polling consumer (`session_get_rgba`/`session_next_rgba`, called
        // from a separate JNI-side thread) therefore almost never observed `valid == true` — it
        // would have to race into the handful of nanoseconds between the two writes. Confirmed
        // on-device: frames were decoding (fps counter updating) but essentially never drawn.
        // Fix: only discard the frame here if NO session handler exists at all (a truly orphaned
        // frame nobody could ever consume) — if one exists (headless or not), leave `valid` as the
        // decoder set it, so a polling consumer without an event stream still gets to read it and
        // is responsible for invalidating it itself via `session_next_rgba` once actually consumed.
        if !is_sent && self.session_handlers.read().unwrap().is_empty() {
            if let Some(rgba_data) = self.display_rgbas.write().unwrap().get_mut(&display) {
                rgba_data.valid = false;
            }
        }
    }

    #[inline]
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    fn on_rgba_flutter_texture_render(
        &self,
        use_texture_render: bool,
        display: usize,
        rgba: &mut scrap::ImageRgb,
    ) {
        for (_, session) in self.session_handlers.read().unwrap().iter() {
            if use_texture_render || session.displays.len() > 1 {
                if session.renderer.on_rgba(display, rgba) {
                    if let Some(stream) = &session.event_stream {
                        stream.add(EventToUI::Texture(display, false));
                    }
                }
            }
        }
    }
}

// This function is only used for the default connection session.
pub fn session_add_existed(
    peer_id: String,
    session_id: SessionID,
    displays: Vec<i32>,
    is_view_camera: bool,
) -> ResultType<()> {
    let conn_type = if is_view_camera {
        ConnType::VIEW_CAMERA
    } else {
        ConnType::DEFAULT_CONN
    };
    sessions::insert_peer_session_id(peer_id, conn_type, session_id, displays);
    Ok(())
}

/// Create a new remote session with the given id.
///
/// # Arguments
///
/// * `id` - The identifier of the remote session with prefix. Regex: [\w]*[\_]*[\d]+
/// * `is_file_transfer` - If the session is used for file transfer.
/// * `is_view_camera` - If the session is used for view camera.
/// * `is_port_forward` - If the session is used for port forward.
pub fn session_add(
    session_id: &SessionID,
    id: &str,
    is_file_transfer: bool,
    is_view_camera: bool,
    is_port_forward: bool,
    is_rdp: bool,
    is_terminal: bool,
    switch_uuid: &str,
    force_relay: bool,
    password: String,
    is_shared_password: bool,
    conn_token: Option<String>,
) -> ResultType<FlutterSession> {
    let conn_type = if is_file_transfer {
        ConnType::FILE_TRANSFER
    } else if is_view_camera {
        ConnType::VIEW_CAMERA
    } else if is_terminal {
        ConnType::TERMINAL
    } else if is_port_forward {
        if is_rdp {
            ConnType::RDP
        } else {
            ConnType::PORT_FORWARD
        }
    } else {
        ConnType::DEFAULT_CONN
    };

    // to-do: check the same id session.
    if let Some(session) = sessions::get_session_by_session_id(&session_id) {
        if session.lc.read().unwrap().conn_type != conn_type {
            bail!("same session id is found with different conn type?");
        }
        // The same session is added before?
        bail!("same session id is found");
    }

    LocalConfig::set_remote_id(&id);

    let mut preset_password = password.clone();
    let shared_password = if is_shared_password {
        // To achieve a flexible password application order, we don't treat shared password as a preset password.
        preset_password = Default::default();
        Some(password)
    } else {
        None
    };

    let session: Session<FlutterHandler> = Session {
        password: preset_password,
        server_keyboard_enabled: Arc::new(RwLock::new(true)),
        server_file_transfer_enabled: Arc::new(RwLock::new(true)),
        server_clipboard_enabled: Arc::new(RwLock::new(true)),
        reconnect_count: Arc::new(AtomicUsize::new(0)),
        ..Default::default()
    };

    let switch_uuid = if switch_uuid.is_empty() {
        None
    } else {
        Some(switch_uuid.to_string())
    };

    session.lc.write().unwrap().initialize(
        id.to_owned(),
        conn_type,
        switch_uuid,
        force_relay,
        get_adapter_luid(),
        shared_password,
        conn_token,
    );

    let session = Arc::new(session.clone());
    sessions::insert_session(session_id.to_owned(), conn_type, session.clone());

    Ok(session)
}

/// start a session with the given id.
///
/// # Arguments
///
/// * `id` - The identifier of the remote session with prefix. Regex: [\w]*[\_]*[\d]+
/// * `events2ui` - The events channel to ui.
pub fn session_start_(
    session_id: &SessionID,
    id: &str,
    event_stream: StreamSink<EventToUI>,
) -> ResultType<()> {
    // is_connected is used to indicate whether to start a peer connection. For two cases:
    // 1. "Move tab to new window"
    // 2. multi ui session within the same peer connection.
    let mut is_connected = false;
    let mut is_found = false;
    for s in sessions::get_sessions() {
        if let Some(h) = s.session_handlers.write().unwrap().get_mut(session_id) {
            is_connected = h.event_stream.is_some();
            try_send_close_event(&h.event_stream);
            h.event_stream = Some(event_stream);
            is_found = true;
            break;
        }
    }
    if !is_found {
        bail!(
            "No session with peer id {}, session id: {}",
            id,
            session_id.to_string()
        );
    }

    if let Some(session) = sessions::get_session_by_session_id(session_id) {
        let is_first_ui_session = session.session_handlers.read().unwrap().len() == 1;
        if !is_connected && is_first_ui_session {
            log::info!(
                "Session {} start, use texture render: {}",
                id,
                session.use_texture_render.load(Ordering::Relaxed)
            );
            let session = (*session).clone();
            std::thread::spawn(move || {
                let round = session.connection_round_state.lock().unwrap().new_round();
                io_loop(session, round);
            });
        }
        Ok(())
    } else {
        bail!("No session with peer id {}", id)
    }
}

/// M2 (plans/soft-frolicking-thimble.md): small additive function for the android-shim JNI
/// plugin, which has no Dart isolate and therefore cannot construct a real `StreamSink<EventToUI>`
/// (it wraps a Dart `MessagePort` obtained from `Dart_NewNativePort_DL`, which requires an actual
/// running Dart VM to have called `Dart_InitializeApiDL` — there is none here). This mirrors
/// `session_start_` exactly except it never touches `h.event_stream` (left as whatever
/// `session_add` already set it to, i.e. `None`) — `io_loop` and everything else already treat a
/// `None` event_stream as "nothing to push UI events to" and skip sending, so leaving it unset is
/// safe. Event delivery for a JNI-only caller is done by polling sync functions instead (e.g.
/// `session_get_rgba`/`session_next_rgba` for video, once M3 gets there).
pub fn session_start_headless(session_id: &SessionID, id: &str) -> ResultType<()> {
    let mut is_connected = false;
    let mut is_found = false;
    for s in sessions::get_sessions() {
        if let Some(h) = s.session_handlers.write().unwrap().get_mut(session_id) {
            is_connected = h.event_stream.is_some();
            is_found = true;
            break;
        }
    }
    if !is_found {
        bail!(
            "No session with peer id {}, session id: {}",
            id,
            session_id.to_string()
        );
    }

    if let Some(session) = sessions::get_session_by_session_id(session_id) {
        let is_first_ui_session = session.session_handlers.read().unwrap().len() == 1;
        if !is_connected && is_first_ui_session {
            log::info!(
                "Session {} start (headless), use texture render: {}",
                id,
                session.use_texture_render.load(Ordering::Relaxed)
            );
            let session = (*session).clone();
            std::thread::spawn(move || {
                let round = session.connection_round_state.lock().unwrap().new_round();
                io_loop(session, round);
            });
        }
        Ok(())
    } else {
        bail!("No session with peer id {}", id)
    }
}

#[inline]
fn try_send_close_event(event_stream: &Option<StreamSink<EventToUI>>) {
    if let Some(stream) = &event_stream {
        stream.add(EventToUI::Event("close".to_owned()));
    }
}

#[cfg(not(target_os = "ios"))]
pub fn update_text_clipboard_required() {
    let is_required = sessions::get_sessions()
        .iter()
        .any(|s| s.is_default() && s.is_text_clipboard_required());
    #[cfg(target_os = "android")]
    let _ = scrap::android::ffi::call_clipboard_manager_enable_client_clipboard(is_required);
    Client::set_is_text_clipboard_required(is_required);
}

#[cfg(feature = "unix-file-copy-paste")]
pub fn update_file_clipboard_required() {
    let is_required = sessions::get_sessions()
        .iter()
        .any(|s| s.is_default() && s.is_file_clipboard_required());
    Client::set_is_file_clipboard_required(is_required);
}

#[cfg(not(target_os = "ios"))]
pub fn send_clipboard_msg(msg: Message, _is_file: bool) {
    for s in sessions::get_sessions() {
        if !s.is_default() {
            continue;
        }
        #[cfg(feature = "unix-file-copy-paste")]
        if _is_file {
            if crate::is_support_file_copy_paste_num(s.lc.read().unwrap().version)
                && s.is_file_clipboard_required()
            {
                s.send(Data::Message(msg.clone()));
            }
            continue;
        }
        if s.is_text_clipboard_required() {
            // Check if the client supports multi clipboards
            if let Some(message::Union::MultiClipboards(multi_clipboards)) = &msg.union {
                let version = s.ui_handler.peer_info.read().unwrap().version.clone();
                let platform = s.ui_handler.peer_info.read().unwrap().platform.clone();
                if let Some(msg_out) = crate::clipboard::get_msg_if_not_support_multi_clip(
                    &version,
                    &platform,
                    multi_clipboards,
                ) {
                    s.send(Data::Message(msg_out));
                    continue;
                }
            }
            s.send(Data::Message(msg.clone()));
        }
    }
}

// Server Side
#[cfg(not(any(target_os = "ios")))]
pub mod connection_manager {
    use std::collections::HashMap;

    #[cfg(any(target_os = "android"))]
    use hbb_common::log;
    #[cfg(any(target_os = "android"))]
    use scrap::android::call_main_service_set_by_name;
    use serde_json::json;

    use crate::ui_cm_interface::InvokeUiCM;

    use super::GLOBAL_EVENT_STREAM;

    #[derive(Clone)]
    struct FlutterHandler {}

    impl InvokeUiCM for FlutterHandler {
        //TODO port_forward
        fn add_connection(&self, client: &crate::ui_cm_interface::Client) {
            let client_json = serde_json::to_string(&client).unwrap_or("".into());
            // send to Android service, active notification no matter UI is shown or not.
            #[cfg(target_os = "android")]
            if let Err(e) =
                call_main_service_set_by_name("add_connection", Some(&client_json), None)
            {
                log::debug!("call_main_service_set_by_name fail,{}", e);
            }
            // send to UI, refresh widget
            self.push_event("add_connection", &[("client", &client_json)]);
        }

        fn remove_connection(&self, id: i32, close: bool) {
            self.push_event(
                "on_client_remove",
                &[("id", &id.to_string()), ("close", &close.to_string())],
            );
        }

        fn new_message(&self, id: i32, text: String) {
            self.push_event(
                "chat_server_mode",
                &[("id", &id.to_string()), ("text", &text)],
            );
        }

        fn change_theme(&self, dark: String) {
            self.push_event("theme", &[("dark", &dark)]);
        }

        fn change_language(&self) {
            self.push_event::<&str>("language", &[]);
        }

        fn show_elevation(&self, show: bool) {
            self.push_event("show_elevation", &[("show", &show.to_string())]);
        }

        fn update_voice_call_state(&self, client: &crate::ui_cm_interface::Client) {
            let client_json = serde_json::to_string(&client).unwrap_or("".into());
            // send to Android service, active notification no matter UI is shown or not.
            #[cfg(target_os = "android")]
            if let Err(e) =
                call_main_service_set_by_name("update_voice_call_state", Some(&client_json), None)
            {
                log::debug!("call_main_service_set_by_name fail,{}", e);
            }
            self.push_event("update_voice_call_state", &[("client", &client_json)]);
        }

        fn file_transfer_log(&self, action: &str, log: &str) {
            self.push_event("cm_file_transfer_log", &[(action, log)]);
        }
    }

    impl FlutterHandler {
        fn push_event<V>(&self, name: &str, event: &[(&str, V)])
        where
            V: Sized + serde::Serialize + Clone,
        {
            let mut h: HashMap<&str, serde_json::Value> =
                event.iter().map(|(k, v)| (*k, json!(*v))).collect();
            debug_assert!(h.get("name").is_none());
            h.insert("name", json!(name));

            if let Some(s) = GLOBAL_EVENT_STREAM.read().unwrap().get(super::APP_TYPE_CM) {
                s.add(serde_json::ser::to_string(&h).unwrap_or("".to_owned()));
            } else {
                println!(
                    "Push event {} failed. No {} event stream found.",
                    name,
                    super::APP_TYPE_CM
                );
            };
        }
    }

    #[inline]
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    pub fn start_cm_no_ui() {
        start_listen_ipc(false);
    }

    #[inline]
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    fn start_listen_ipc_thread() {
        start_listen_ipc(true);
    }

    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    fn start_listen_ipc(new_thread: bool) {
        use crate::ui_cm_interface::{start_ipc, ConnectionManager};

        #[cfg(target_os = "linux")]
        std::thread::spawn(crate::ipc::start_pa);

        let cm = ConnectionManager {
            ui_handler: FlutterHandler {},
        };
        if new_thread {
            std::thread::spawn(move || start_ipc(cm));
        } else {
            start_ipc(cm);
        }
    }

    #[inline]
    pub fn cm_init() {
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        start_listen_ipc_thread();
    }

    #[cfg(target_os = "android")]
    use hbb_common::tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

    #[cfg(target_os = "android")]
    pub fn start_channel(
        rx: UnboundedReceiver<crate::ipc::Data>,
        tx: UnboundedSender<crate::ipc::Data>,
    ) {
        use crate::ui_cm_interface::start_listen;
        let cm = crate::ui_cm_interface::ConnectionManager {
            ui_handler: FlutterHandler {},
        };
        std::thread::spawn(move || start_listen(cm, rx, tx));
    }
}

pub fn make_fd_flutter(id: i32, entries: &Vec<FileEntry>, only_count: bool) -> String {
    let mut m = serde_json::Map::new();
    m.insert("id".into(), json!(id));
    let mut a = vec![];
    let mut n: u64 = 0;
    for entry in entries {
        n += entry.size;
        if only_count {
            continue;
        }
        let mut e = serde_json::Map::new();
        e.insert("name".into(), json!(entry.name.to_owned()));
        let tmp = entry.entry_type.value();
        e.insert("type".into(), json!(if tmp == 0 { 1 } else { tmp }));
        e.insert("time".into(), json!(entry.modified_time as f64));
        e.insert("size".into(), json!(entry.size as f64));
        a.push(e);
    }
    if only_count {
        m.insert("num_entries".into(), json!(entries.len() as i32));
    } else {
        m.insert("entries".into(), json!(a));
    }
    m.insert("total_size".into(), json!(n as f64));
    serde_json::to_string(&m).unwrap_or("".into())
}

pub fn get_cur_session_id() -> SessionID {
    CUR_SESSION_ID.read().unwrap().clone()
}

pub fn get_cur_peer_id() -> String {
    sessions::get_peer_id_by_session_id(&get_cur_session_id(), ConnType::DEFAULT_CONN)
        .unwrap_or("".to_string())
}

pub fn set_cur_session_id(session_id: SessionID) {
    if get_cur_session_id() != session_id {
        *CUR_SESSION_ID.write().unwrap() = session_id;
    }
}

#[inline]
fn serialize_resolutions(resolutions: &Vec<Resolution>) -> String {
    #[derive(Debug, serde::Serialize)]
    struct ResolutionSerde {
        width: i32,
        height: i32,
    }

    let mut v = vec![];
    resolutions
        .iter()
        .map(|r| {
            v.push(ResolutionSerde {
                width: r.width,
                height: r.height,
            })
        })
        .count();
    serde_json::ser::to_string(&v).unwrap_or("".to_string())
}

fn char_to_session_id(c: *const char) -> ResultType<SessionID> {
    if c.is_null() {
        bail!("Session id ptr is null");
    }
    let cstr = unsafe { std::ffi::CStr::from_ptr(c as _) };
    let str = cstr.to_str()?;
    SessionID::from_str(str).map_err(|e| anyhow!("{:?}", e))
}

pub fn session_get_rgba_size(session_id: SessionID, display: usize) -> usize {
    if let Some(session) = sessions::get_session_by_session_id(&session_id) {
        return session
            .display_rgbas
            .read()
            .unwrap()
            .get(&display)
            .map_or(0, |rgba| rgba.data.len());
    }
    0
}

/// M3 (plans/soft-frolicking-thimble.md): synchronous, pollable (width, height) getter for a
/// headless session's display, added for the same reason as `session_start_headless` — the normal
/// channel for this (`make_displays_msg`, built from `PeerInfo::displays` and pushed through the
/// event stream in `set_peer_info`/`set_displays`) is unavailable since `session_start_headless`
/// never sets `event_stream`. `set_peer_info`/`set_displays` populate `PeerInfo.displays`
/// unconditionally (the event push is just an extra side effect on top), so the same data can be
/// read directly here. Returns (0, 0) if the session or that display index isn't known yet (e.g.
/// called before the peer handshake/first frame).
pub fn session_get_display_size(session_id: SessionID, display: usize) -> (i32, i32) {
    if let Some(session) = sessions::get_session_by_session_id(&session_id) {
        if let Some(d) = session.peer_info.read().unwrap().displays.get(display) {
            return (d.width, d.height);
        }
    }
    (0, 0)
}

/// Multi-monitor origin (lobishell-android): sync, pollable (x, y) getter for a display's position
/// within the peer's COMBINED virtual desktop — same "small additive getter next to
/// session_get_display_size" pattern, for the same headless-session reason. Needed because mouse
/// input ultimately lands via enigo's Windows backend (`mouse_move_to`), which expects absolute
/// virtual-desktop coordinates (offset from `SM_XVIRTUALSCREEN`/`SM_YVIRTUALSCREEN`), not
/// coordinates local to whichever display is currently selected — the Android client was sending
/// local 0..width/0..height coordinates unmodified, which only happened to work for a display
/// whose own origin is (0,0) (typically the primary). A secondary/non-primary display (e.g. one
/// positioned to the right, or a portrait-rotated monitor) has a nonzero origin, and taps sent
/// without adding it landed on whatever display actually occupies that (0,0)-relative position —
/// reported as "mouse moves on the first monitor" while viewing a different one. `DisplayInfo`
/// (message.proto) already carries `x`/`y` for exactly this; `session_get_display_size` simply
/// never read them. Returns (0, 0) if not known yet (peer info not received) — same fallback the
/// paired size getter uses, so a caller that hasn't gotten a real origin yet just adds nothing.
pub fn session_get_display_origin(session_id: SessionID, display: usize) -> (i32, i32) {
    if let Some(session) = sessions::get_session_by_session_id(&session_id) {
        if let Some(d) = session.peer_info.read().unwrap().displays.get(display) {
            return (d.x, d.y);
        }
    }
    (0, 0)
}

/// Multi-monitor (lobishell-android): sync, pollable count of displays the peer reported — same
/// "small additive getter next to session_get_display_size" pattern, for the same headless-session
/// reason (no event_stream to push the count through). Used by the Android client to decide
/// whether to show a display picker at all, and to bound the index passed to
/// flutter_ffi::session_switch_display. Returns 0 if not known yet.
pub fn session_get_display_count(session_id: SessionID) -> i32 {
    if let Some(session) = sessions::get_session_by_session_id(&session_id) {
        return session.peer_info.read().unwrap().displays.len() as i32;
    }
    0
}

/// Privacy mode (plans/soft-frolicking-thimble.md): whether the connected peer advertised support
/// for it at all (`PeerInfo.features.privacy_mode`, the same field RustDesk's own Flutter UI reads
/// via `set_peer_info`'s push_event — `set_peer_info` already unconditionally caches the peer's
/// whole PeerInfo into `session.peer_info` regardless of push_event succeeding, same as
/// `session_get_display_size` above reads `.displays` from the same cache). No new patch needed —
/// `session_toggle_privacy_mode` (below) and the toggle-state getter already exist upstream in
/// `flutter_ffi.rs`/`client.rs`; this is the one missing piece, "does the peer even support it."
pub fn session_is_privacy_mode_supported(session_id: SessionID) -> bool {
    if let Some(session) = sessions::get_session_by_session_id(&session_id) {
        return session
            .peer_info
            .read()
            .unwrap()
            .features
            .iter()
            .next()
            .map(|f| f.privacy_mode)
            .unwrap_or(false);
    }
    false
}

/// The first privacy-mode implementation key the peer advertised supporting (from
/// `PeerInfo.platform_additions`'s own `"supported_privacy_mode_impl": [[impl_key, tip_key], ...]`
/// list — confirmed via a real on-device PeerInfo dump this session, not guessed), or an empty
/// string if none/not yet known. `toggle_privacy_mode` needs a specific impl_key — RustDesk's own
/// Flutter UI lets the user pick among several when more than one is offered; this headless client
/// just uses whichever the peer lists first, a reasonable default absent any UI to choose
/// otherwise.
pub fn session_default_privacy_mode_impl(session_id: SessionID) -> String {
    let Some(session) = sessions::get_session_by_session_id(&session_id) else {
        return String::new();
    };
    let platform_additions = session.peer_info.read().unwrap().platform_additions.clone();
    let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&platform_additions) else {
        return String::new();
    };
    parsed
        .get("supported_privacy_mode_impl")
        .and_then(|v| v.as_array())
        .and_then(|arr| arr.first())
        .and_then(|entry| entry.as_array())
        .and_then(|pair| pair.first())
        .and_then(|s| s.as_str())
        .unwrap_or("")
        .to_string()
}

/// View Camera (plans/soft-frolicking-thimble.md): whether the connected peer advertised a camera
/// to view, via `PeerInfo.platform_additions`'s own `"support_view_camera": true/false` flag (same
/// cache as `session_default_privacy_mode_impl` above reads its own key from — `set_peer_info`
/// unconditionally populates `session.peer_info` for every headless session regardless of
/// push_event). Meant to be read on the already-open CONTROL session (not a dedicated view-camera
/// session, which won't have peer_info populated until it's connected) to decide whether to show a
/// "View Camera" entry point at all before opening the second, dedicated session.
pub fn session_is_view_camera_supported(session_id: SessionID) -> bool {
    let Some(session) = sessions::get_session_by_session_id(&session_id) else {
        return false;
    };
    let platform_additions = session.peer_info.read().unwrap().platform_additions.clone();
    let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&platform_additions) else {
        return false;
    };
    parsed
        .get("support_view_camera")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

/// Pre-connect online check (lobishell-android): queries the rendezvous server directly for
/// whether `id` is currently online, without needing a session at all. RustDesk's own public
/// `client::peer_online::query_online_states` wrapper delivers its result through
/// `start_flutter_async_runner` + a global Flutter event stream — plumbing our headless client
/// never sets up (same reason `session_start_headless` exists). This instead blocks on the
/// lower-level `query_online_states_` primitive directly (now `pub(crate)`, see its doc), which
/// already returns the (onlines, offlines) pair with no callback needed. Uses whatever
/// custom-server config was last set via `main_set_option` (same as a real connect). Blocks the
/// calling thread for up to ~3s (the query's own internal timeout) — call off the UI thread.
/// Returns 1 = online, 0 = offline, -1 = unknown (query itself failed, e.g. no network reachable
/// to the rendezvous server) — callers should treat -1 as "don't block the normal connect flow",
/// not as a definite offline.
pub fn session_is_id_online(id: String) -> i32 {
    let rt = match hbb_common::tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(_) => return -1,
    };
    let ids = vec![id.clone()];
    match rt.block_on(crate::client::peer_online::query_online_states_(
        &ids,
        std::time::Duration::from_millis(3_000),
    )) {
        Ok((onlines, offlines)) => {
            if onlines.contains(&id) {
                1
            } else if offlines.contains(&id) {
                0
            } else {
                -1
            }
        }
        Err(_) => -1,
    }
}

#[no_mangle]
pub extern "C" fn session_get_rgba(session_uuid_str: *const char, display: usize) -> *const u8 {
    if let Ok(session_id) = char_to_session_id(session_uuid_str) {
        if let Some(s) = sessions::get_session_by_session_id(&session_id) {
            return s.ui_handler.get_rgba(display);
        }
    }

    std::ptr::null()
}

pub fn session_next_rgba(session_id: SessionID, display: usize) {
    if let Some(s) = sessions::get_session_by_session_id(&session_id) {
        return s.ui_handler.next_rgba(display);
    }
}

#[inline]
pub fn session_set_size(session_id: SessionID, display: usize, width: usize, height: usize) {
    for s in sessions::get_sessions() {
        if let Some(h) = s
            .ui_handler
            .session_handlers
            .write()
            .unwrap()
            .get_mut(&session_id)
        {
            // If the session is the first connection, displays is not set yet.
            // `displays`` is set while switching displays or adding a new session.
            if !h.displays.contains(&display) {
                h.displays.push(display);
            }
            h.renderer.set_size(display, width, height);
            break;
        }
    }
}

#[inline]
pub fn session_register_pixelbuffer_texture(session_id: SessionID, display: usize, ptr: usize) {
    for s in sessions::get_sessions() {
        if let Some(h) = s
            .ui_handler
            .session_handlers
            .read()
            .unwrap()
            .get(&session_id)
        {
            h.renderer.register_pixelbuffer_texture(display, ptr);
            break;
        }
    }
}

#[inline]
pub fn session_register_gpu_texture(_session_id: SessionID, _display: usize, _output_ptr: usize) {
    #[cfg(feature = "vram")]
    for s in sessions::get_sessions() {
        if let Some(h) = s
            .ui_handler
            .session_handlers
            .read()
            .unwrap()
            .get(&_session_id)
        {
            h.renderer.register_gpu_output(_display, _output_ptr);
            break;
        }
    }
}

#[inline]
#[cfg(not(feature = "vram"))]
pub fn get_adapter_luid() -> Option<i64> {
    None
}

#[cfg(feature = "vram")]
pub fn get_adapter_luid() -> Option<i64> {
    if !crate::ui_interface::use_texture_render() {
        return None;
    }
    let get_adapter_luid_func = match &*TEXTURE_GPU_RENDERER_PLUGIN {
        Ok(lib) => {
            let find_sym_res = unsafe {
                lib.symbol::<FlutterGpuTextureRendererPluginCApiGetAdapterLuid>(
                    "FlutterGpuTextureRendererPluginCApiGetAdapterLuid",
                )
            };
            match find_sym_res {
                Ok(sym) => Some(sym),
                Err(e) => {
                    log::error!("Failed to find symbol FlutterGpuTextureRendererPluginCApiGetAdapterLuid, {e}");
                    None
                }
            }
        }
        Err(e) => {
            log::error!("Failed to load texture gpu renderer plugin, {e}");
            None
        }
    };
    let adapter_luid = match get_adapter_luid_func {
        Some(get_adapter_luid_func) => unsafe { Some(get_adapter_luid_func()) },
        None => Default::default(),
    };
    return adapter_luid;
}

#[inline]
pub fn push_session_event(session_id: &SessionID, name: &str, event: Vec<(&str, &str)>) {
    if let Some(s) = sessions::get_session_by_session_id(session_id) {
        s.push_event(name, &event, &[]);
    }
}

#[inline]
pub fn push_global_event(channel: &str, event: String) -> Option<bool> {
    Some(GLOBAL_EVENT_STREAM.read().unwrap().get(channel)?.add(event))
}

#[inline]
pub fn get_global_event_channels() -> Vec<String> {
    GLOBAL_EVENT_STREAM
        .read()
        .unwrap()
        .keys()
        .cloned()
        .collect()
}

pub fn start_global_event_stream(s: StreamSink<String>, app_type: String) -> ResultType<()> {
    let app_type_values = app_type.split(",").collect::<Vec<&str>>();
    let mut lock = GLOBAL_EVENT_STREAM.write().unwrap();
    if !lock.contains_key(app_type_values[0]) {
        lock.insert(app_type_values[0].to_string(), s);
    } else {
        if let Some(_) = lock.insert(app_type.clone(), s) {
            log::warn!(
                "Global event stream of type {} is started before, but now removed",
                app_type
            );
        }
    }
    Ok(())
}

pub fn stop_global_event_stream(app_type: String) {
    let _ = GLOBAL_EVENT_STREAM.write().unwrap().remove(&app_type);
}

#[inline]
fn session_send_touch_scale(
    session_id: SessionID,
    v: &serde_json::Value,
    alt: bool,
    ctrl: bool,
    shift: bool,
    command: bool,
) {
    match v.get("v").and_then(|s| s.as_i64()) {
        Some(scale) => {
            if let Some(session) = sessions::get_session_by_session_id(&session_id) {
                session.send_touch_scale(scale as _, alt, ctrl, shift, command);
            }
        }
        None => {}
    }
}

#[inline]
fn session_send_touch_pan(
    session_id: SessionID,
    v: &serde_json::Value,
    pan_event: &str,
    alt: bool,
    ctrl: bool,
    shift: bool,
    command: bool,
) {
    match v.get("v") {
        Some(v) => match (
            v.get("x").and_then(|x| x.as_i64()),
            v.get("y").and_then(|y| y.as_i64()),
        ) {
            (Some(x), Some(y)) => {
                if let Some(session) = sessions::get_session_by_session_id(&session_id) {
                    session
                        .send_touch_pan_event(pan_event, x as _, y as _, alt, ctrl, shift, command);
                }
            }
            _ => {}
        },
        _ => {}
    }
}

fn session_send_touch_event(
    session_id: SessionID,
    v: &serde_json::Value,
    alt: bool,
    ctrl: bool,
    shift: bool,
    command: bool,
) {
    match v.get("t").and_then(|t| t.as_str()) {
        Some("scale") => session_send_touch_scale(session_id, v, alt, ctrl, shift, command),
        Some(pan_event) => {
            session_send_touch_pan(session_id, v, pan_event, alt, ctrl, shift, command)
        }
        _ => {}
    }
}

pub fn session_send_pointer(session_id: SessionID, msg: String) {
    if let Ok(m) = serde_json::from_str::<HashMap<String, serde_json::Value>>(&msg) {
        let alt = m.get("alt").is_some();
        let ctrl = m.get("ctrl").is_some();
        let shift = m.get("shift").is_some();
        let command = m.get("command").is_some();
        match (m.get("k"), m.get("v")) {
            (Some(k), Some(v)) => match k.as_str() {
                Some("touch") => session_send_touch_event(session_id, v, alt, ctrl, shift, command),
                _ => {}
            },
            _ => {}
        }
    }
}

#[inline]
pub fn session_on_waiting_for_image_dialog_show(session_id: SessionID) {
    for s in sessions::get_sessions() {
        if let Some(h) = s.session_handlers.write().unwrap().get_mut(&session_id) {
            h.on_waiting_for_image_dialog_show();
        }
    }
}

/// Hooks for session.
#[derive(Clone)]
pub enum SessionHook {
    OnSessionRgba(fn(String, &mut scrap::ImageRgb)),
}

#[inline]
pub fn get_cur_session() -> Option<FlutterSession> {
    sessions::get_session_by_session_id(&*CUR_SESSION_ID.read().unwrap())
}

#[inline]
pub fn try_sync_peer_option(
    session: &FlutterSession,
    cur_id: &SessionID,
    key: &str,
    _value: Option<serde_json::Value>,
) {
    let mut event = Vec::new();
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    if key == "view-only" {
        event = vec![
            ("k", json!(key.to_string())),
            ("v", json!(session.lc.read().unwrap().view_only.v)),
        ];
    }
    if ["keyboard_mode", "input_source"].contains(&key) {
        event = vec![("k", json!(key.to_string())), ("v", json!(""))];
    }
    if !event.is_empty() {
        session.push_event("sync_peer_option", &event, &[cur_id]);
    }
}

pub(super) fn session_update_virtual_display(session: &FlutterSession, index: i32, on: bool) {
    let virtual_display_key = "virtual-display";
    let displays = session.get_option(virtual_display_key.to_owned());
    if !on {
        if index == -1 {
            if !displays.is_empty() {
                session.set_option(virtual_display_key.to_owned(), "".to_owned());
            }
        } else {
            let mut vdisplays = displays.split(',').collect::<Vec<_>>();
            let len = vdisplays.len();
            if index == 0 {
                // 0 means we can't toggle the virtual display by index.
                vdisplays.remove(vdisplays.len() - 1);
            } else {
                if let Some(i) = vdisplays.iter().position(|&x| x == index.to_string()) {
                    vdisplays.remove(i);
                }
            }
            if vdisplays.len() != len {
                session.set_option(
                    virtual_display_key.to_owned(),
                    vdisplays.join(",").to_owned(),
                );
            }
        }
    } else {
        let mut vdisplays = displays
            .split(',')
            .map(|x| x.to_string())
            .collect::<Vec<_>>();
        let len = vdisplays.len();
        if index == 0 {
            vdisplays.push(index.to_string());
        } else {
            if !vdisplays.iter().any(|x| *x == index.to_string()) {
                vdisplays.push(index.to_string());
            }
        }
        if vdisplays.len() != len {
            session.set_option(
                virtual_display_key.to_owned(),
                vdisplays.join(",").to_owned(),
            );
        }
    }
}

// sessions mod is used to avoid the big lock of sessions' map.
pub mod sessions {

    use super::*;

    lazy_static::lazy_static! {
        // peer -> peer session, peer session -> ui sessions
        static ref SESSIONS: RwLock<HashMap<(String, ConnType), FlutterSession>> = Default::default();
    }

    #[inline]
    pub fn get_session_count(peer_id: String, conn_type: ConnType) -> usize {
        SESSIONS
            .read()
            .unwrap()
            .get(&(peer_id, conn_type))
            .map(|s| s.ui_handler.session_handlers.read().unwrap().len())
            .unwrap_or(0)
    }

    #[inline]
    pub fn get_peer_id_by_session_id(id: &SessionID, conn_type: ConnType) -> Option<String> {
        SESSIONS
            .read()
            .unwrap()
            .iter()
            .find_map(|((peer_id, t), s)| {
                if *t == conn_type
                    && s.ui_handler
                        .session_handlers
                        .read()
                        .unwrap()
                        .contains_key(id)
                {
                    Some(peer_id.clone())
                } else {
                    None
                }
            })
    }

    #[inline]
    pub fn get_session_by_session_id(id: &SessionID) -> Option<FlutterSession> {
        SESSIONS
            .read()
            .unwrap()
            .values()
            .find(|s| {
                s.ui_handler
                    .session_handlers
                    .read()
                    .unwrap()
                    .contains_key(id)
            })
            .cloned()
    }

    #[inline]
    pub fn get_session_by_peer_id(peer_id: String, conn_type: ConnType) -> Option<FlutterSession> {
        SESSIONS.read().unwrap().get(&(peer_id, conn_type)).cloned()
    }

    #[inline]
    pub fn remove_session_by_session_id(id: &SessionID) -> Option<FlutterSession> {
        let mut remove_peer_key = None;
        for (peer_key, s) in SESSIONS.write().unwrap().iter_mut() {
            let mut write_lock = s.ui_handler.session_handlers.write().unwrap();
            let remove_ret = write_lock.remove(id);
            match remove_ret {
                Some(_) => {
                    if write_lock.is_empty() {
                        remove_peer_key = Some(peer_key.clone());
                    } else {
                        check_remove_unused_displays(None, id, s, &write_lock);
                    }
                    break;
                }
                None => {}
            }
        }
        let s = SESSIONS.write().unwrap().remove(&remove_peer_key?);
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        update_session_count_to_server();
        s
    }

    /// Check if removing a session by session_id would result in removing the entire peer.
    ///
    /// Returns:
    /// - `true`: The session exists and removing it would leave the peer with no other sessions,
    ///           so the entire peer would be removed (equivalent to `remove_session_by_session_id` returning `Some`)
    /// - `false`: The session doesn't exist, or it exists but the peer has other sessions,
    ///            so the peer would not be removed (equivalent to `remove_session_by_session_id` returning `None`)
    #[inline]
    pub fn would_remove_peer_by_session_id(id: &SessionID) -> bool {
        for (_peer_key, s) in SESSIONS.read().unwrap().iter() {
            let read_lock = s.ui_handler.session_handlers.read().unwrap();
            if read_lock.contains_key(id) {
                // Found the session, check if it's the only one for this peer
                return read_lock.len() == 1;
            }
        }
        // Session not found
        false
    }

    fn check_remove_unused_displays(
        current: Option<usize>,
        session_id: &SessionID,
        session: &FlutterSession,
        handlers: &HashMap<SessionID, SessionHandler>,
    ) {
        // Set capture displays if some are not used any more.
        let mut remains_displays = HashSet::new();
        if let Some(current) = current {
            remains_displays.insert(current);
        }
        for (k, h) in handlers.iter() {
            if k == session_id {
                continue;
            }
            remains_displays.extend(
                h.renderer
                    .map_display_sessions
                    .read()
                    .unwrap()
                    .keys()
                    .cloned(),
            );
        }
        if !remains_displays.is_empty() {
            session.capture_displays(
                vec![],
                vec![],
                remains_displays.iter().map(|d| *d as i32).collect(),
            );
        }
    }

    pub fn session_switch_display(is_desktop: bool, session_id: SessionID, value: Vec<i32>) {
        for s in SESSIONS.read().unwrap().values() {
            let mut write_lock = s.ui_handler.session_handlers.write().unwrap();
            if let Some(h) = write_lock.get_mut(&session_id) {
                h.displays = value.iter().map(|x| *x as usize).collect::<_>();
                #[cfg(not(any(target_os = "android", target_os = "ios")))]
                let displays_refresh = value.clone();
                if value.len() == 1 {
                    // Switch display.
                    // This operation will also cause the peer to send a switch display message.
                    // The switch display message will contain `SupportedResolutions`, which is useful when changing resolutions.
                    s.switch_display(value[0]);
                    // Reset the valid flag of the display.
                    s.next_rgba(value[0] as usize);

                    if !is_desktop {
                        s.capture_displays(vec![], vec![], value);
                    } else {
                        // Check if other displays are needed.
                        if value.len() == 1 {
                            check_remove_unused_displays(
                                Some(value[0] as _),
                                &session_id,
                                &s,
                                &write_lock,
                            );
                        }
                    }
                } else {
                    // Try capture all displays.
                    s.capture_displays(vec![], vec![], value);
                }
                // When switching display, we also need to send "Refresh display" message.
                // On the controlled side:
                // 1. If this display is not currently captured -> Refresh -> Message "Refresh display" is not required.
                // One more key frame (first frame) will be sent because the refresh message.
                // 2. If this display is currently captured -> Not refresh -> Message "Refresh display" is required.
                // Without the message, the control side cannot see the latest display image.
                #[cfg(not(any(target_os = "android", target_os = "ios")))]
                {
                    let is_support_multi_ui_session = crate::common::is_support_multi_ui_session(
                        &s.ui_handler.peer_info.read().unwrap().version,
                    );
                    if is_support_multi_ui_session {
                        for display in displays_refresh.iter() {
                            s.refresh_video(*display);
                        }
                    }
                }
                break;
            }
        }
    }

    #[inline]
    pub fn insert_session(session_id: SessionID, conn_type: ConnType, session: FlutterSession) {
        SESSIONS
            .write()
            .unwrap()
            .entry((session.get_id(), conn_type))
            .or_insert(session)
            .ui_handler
            .session_handlers
            .write()
            .unwrap()
            .insert(session_id, Default::default());
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        update_session_count_to_server();
    }

    #[inline]
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    fn update_session_count_to_server() {
        crate::ipc::update_controlling_session_count(SESSIONS.read().unwrap().len()).ok();
    }

    #[inline]
    pub fn insert_peer_session_id(
        peer_id: String,
        conn_type: ConnType,
        session_id: SessionID,
        displays: Vec<i32>,
    ) -> bool {
        if let Some(s) = SESSIONS.read().unwrap().get(&(peer_id, conn_type)) {
            let mut h = SessionHandler::default();
            h.displays = displays.iter().map(|x| *x as usize).collect::<_>();
            #[cfg(not(any(target_os = "android", target_os = "ios")))]
            let is_support_multi_ui_session = crate::common::is_support_multi_ui_session(
                &s.ui_handler.peer_info.read().unwrap().version,
            );
            #[cfg(any(target_os = "android", target_os = "ios"))]
            let is_support_multi_ui_session = false;
            h.renderer.is_support_multi_ui_session = is_support_multi_ui_session;
            let _ = s
                .ui_handler
                .session_handlers
                .write()
                .unwrap()
                .insert(session_id, h);
            // If the session is a single display session, it may be a software rgba rendered display.
            // If this is the second time the display is opened, the old valid flag may be true.
            if displays.len() == 1 {
                s.ui_handler.next_rgba(displays[0] as usize);
            }
            true
        } else {
            false
        }
    }

    #[inline]
    pub fn get_sessions() -> Vec<FlutterSession> {
        SESSIONS.read().unwrap().values().cloned().collect()
    }

    #[inline]
    #[cfg(not(target_os = "ios"))]
    pub fn has_sessions_running(conn_type: ConnType) -> bool {
        SESSIONS.read().unwrap().iter().any(|((_, r#type), s)| {
            *r#type == conn_type && s.session_handlers.read().unwrap().len() != 0
        })
    }

    #[inline]
    #[cfg(not(target_os = "ios"))]
    pub fn has_connected_sessions_running(conn_type: ConnType) -> bool {
        SESSIONS.read().unwrap().iter().any(|((_, r#type), s)| {
            *r#type == conn_type
                && s.session_handlers.read().unwrap().len() != 0
                && s.connection_round_state.lock().unwrap().is_connected()
        })
    }
}

pub(super) mod async_tasks {
    use hbb_common::{bail, tokio, ResultType};
    use std::{
        collections::HashMap,
        sync::{
            mpsc::{sync_channel, SyncSender},
            Arc, Mutex,
        },
    };

    type TxQueryOnlines = SyncSender<Vec<String>>;
    lazy_static::lazy_static! {
        static ref TX_QUERY_ONLINES: Arc<Mutex<Option<TxQueryOnlines>>> = Default::default();
    }

    #[inline]
    pub fn start_flutter_async_runner() {
        std::thread::spawn(start_flutter_async_runner_);
    }

    #[allow(dead_code)]
    pub fn stop_flutter_async_runner() {
        let _ = TX_QUERY_ONLINES.lock().unwrap().take();
    }

    #[tokio::main(flavor = "current_thread")]
    async fn start_flutter_async_runner_() {
        // Only one task is allowed to run at the same time.
        let (tx_onlines, rx_onlines) = sync_channel::<Vec<String>>(1);
        TX_QUERY_ONLINES.lock().unwrap().replace(tx_onlines);

        loop {
            match rx_onlines.recv() {
                Ok(ids) => {
                    crate::client::peer_online::query_online_states(ids, handle_query_onlines).await
                }
                _ => {
                    // unreachable!
                    break;
                }
            }
        }
    }

    pub fn query_onlines(ids: Vec<String>) -> ResultType<()> {
        if let Some(tx) = TX_QUERY_ONLINES.lock().unwrap().as_ref() {
            // Ignore if the channel is full.
            let _ = tx.try_send(ids)?;
        } else {
            bail!("No tx_query_onlines");
        }
        Ok(())
    }

    fn handle_query_onlines(onlines: Vec<String>, offlines: Vec<String>) {
        let data = HashMap::from([
            ("name", "callback_query_onlines".to_owned()),
            ("onlines", onlines.join(",")),
            ("offlines", offlines.join(",")),
        ]);
        let _res = super::push_global_event(
            super::APP_TYPE_MAIN,
            serde_json::ser::to_string(&data).unwrap_or("".to_owned()),
        );
    }
}
