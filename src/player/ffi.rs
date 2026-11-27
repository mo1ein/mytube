// Subset of mpv's public client.h, render.h, and render_gl.h ABIs.
use std::{
    ffi::{c_char, c_double, c_int, c_ulong, c_void},
    path::Path,
};

use anyhow::{Context, Result, ensure};
use libloading::Library;

pub type Handle = *mut c_void;
pub type Notify = Option<unsafe extern "C" fn(*mut c_void)>;
pub const STRING: c_int = 1;
pub const FLAG: c_int = 3;
pub const DOUBLE: c_int = 5;
pub const EVENT_NONE: c_int = 0;
pub const EVENT_SHUTDOWN: c_int = 1;
pub const EVENT_START_FILE: c_int = 6;
pub const EVENT_END_FILE: c_int = 7;
pub const EVENT_FILE_LOADED: c_int = 8;
pub const EVENT_SEEK: c_int = 20;
pub const EVENT_PLAYBACK_RESTART: c_int = 21;
pub const EVENT_PROPERTY_CHANGE: c_int = 22;
pub const RENDER_API: c_int = 1;
pub const RENDER_GL_INIT: c_int = 2;
pub const RENDER_GL_FBO: c_int = 3;
pub const RENDER_FLIP_Y: c_int = 4;
pub const RENDER_BLOCK: c_int = 12;

#[repr(C)]
pub struct Event {
    pub id: c_int,
    pub error: c_int,
    pub reply: u64,
    pub data: *const c_void,
}

#[repr(C)]
pub struct Property {
    pub name: *const c_char,
    pub format: c_int,
    pub data: *const c_void,
}

#[repr(C)]
pub struct EndFile {
    pub reason: c_int,
    pub error: c_int,
    pub playlist_entry_id: i64,
}

#[repr(C)]
pub struct RenderParam {
    kind: c_int,
    data: *mut c_void,
}

impl RenderParam {
    pub const END: Self = Self {
        kind: 0,
        data: std::ptr::null_mut(),
    };

    pub fn new<T>(kind: c_int, data: *mut T) -> Self {
        Self {
            kind,
            data: data.cast(),
        }
    }
}

#[repr(C)]
pub struct GlInit {
    pub lookup: unsafe extern "C" fn(*mut c_void, *const c_char) -> *mut c_void,
    pub context: *mut c_void,
}

#[repr(C)]
pub struct Fbo {
    pub id: c_int,
    pub w: c_int,
    pub h: c_int,
    pub format: c_int,
}

macro_rules! api {
    ($($name:ident: $function:ty),* $(,)?) => {
        pub struct Api { _library: Library, $(pub $name: $function,)* }
        impl Api {
            pub fn open(path: &Path) -> Result<Self> {
                // The library stays mapped until all handles and renderer callbacks are dropped.
                let library = unsafe { Library::new(path) }.with_context(|| format!("Could not load {}", path.display()))?;
                $(let $name = unsafe { *library.get::<$function>(concat!(stringify!($name), "\0").as_bytes())? };)*
                let api = Self { _library: library, $($name,)* };
                ensure!((unsafe { (api.mpv_client_api_version)() } >> 16) == 2, "Unsupported libmpv client API");
                Ok(api)
            }
        }
    }
}

api! {
    mpv_client_api_version: unsafe extern "C" fn() -> c_ulong,
    mpv_create: unsafe extern "C" fn() -> Handle,
    mpv_initialize: unsafe extern "C" fn(Handle) -> c_int,
    mpv_terminate_destroy: unsafe extern "C" fn(Handle),
    mpv_set_option_string: unsafe extern "C" fn(Handle, *const c_char, *const c_char) -> c_int,
    mpv_command_async: unsafe extern "C" fn(Handle, u64, *const *const c_char) -> c_int,
    mpv_command: unsafe extern "C" fn(Handle, *const *const c_char) -> c_int,
    mpv_get_property: unsafe extern "C" fn(Handle, *const c_char, c_int, *mut c_void) -> c_int,
    mpv_free: unsafe extern "C" fn(*mut c_void),
    mpv_observe_property: unsafe extern "C" fn(Handle, u64, *const c_char, c_int) -> c_int,
    mpv_set_wakeup_callback: unsafe extern "C" fn(Handle, Notify, *mut c_void),
    mpv_wait_event: unsafe extern "C" fn(Handle, c_double) -> *const Event,
    mpv_error_string: unsafe extern "C" fn(c_int) -> *const c_char,
    mpv_render_context_create: unsafe extern "C" fn(*mut Handle, Handle, *mut RenderParam) -> c_int,
    mpv_render_context_set_update_callback: unsafe extern "C" fn(Handle, Notify, *mut c_void),
    mpv_render_context_render: unsafe extern "C" fn(Handle, *mut RenderParam) -> c_int,
    mpv_render_context_free: unsafe extern "C" fn(Handle),
}
