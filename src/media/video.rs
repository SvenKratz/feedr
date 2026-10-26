//! Minimal libmpv software-render adapter. All mpv calls stay on its worker.
//! API contracts: https://github.com/mpv-player/mpv/blob/v0.40.0/include/mpv/render.h
//! Rendering uses asynchronous commands and observed properties, never blocking
//! core calls on the render thread. No player window or user mpv config is used.
use anyhow::{Context as _, Result, ensure};
use eframe::egui::{self, ColorImage};
use libloading::Library;
use std::{
    ffi::{CStr, CString, c_char, c_int, c_void},
    path::Path,
    ptr,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Debug)]
pub struct Status {
    pub paused: bool,
    pub position: f64,
    pub duration: f64,
    pub volume: f64,
    pub audio: bool,
    pub error: Option<String>,
}
impl Default for Status {
    fn default() -> Self {
        Self {
            paused: true,
            position: 0.,
            duration: 0.,
            volume: 80.,
            audio: false,
            error: None,
        }
    }
}
#[derive(Default)]
struct Output {
    status: Status,
    frame: Option<ColorImage>,
}
pub enum Control {
    Pause(bool),
    Seek(f64),
    Volume(f64),
}
pub struct Player {
    controls: mpsc::Sender<Control>,
    output: Arc<Mutex<Output>>,
    stop: Arc<AtomicBool>,
}
impl Player {
    pub fn new(path: &Path, ctx: egui::Context) -> Self {
        let (controls, receiver) = mpsc::channel();
        let output = Arc::new(Mutex::new(Output::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let (state, stopping, path) = (output.clone(), stop.clone(), path.to_owned());
        thread::spawn(move || {
            if let Err(error) = play(&path, &receiver, &state, &stopping, &ctx) {
                if let Ok(mut output) = state.lock() {
                    output.status.error = Some(format!("{error:#}"));
                }
                ctx.request_repaint();
            }
        });
        Self {
            controls,
            output,
            stop,
        }
    }
    pub fn control(&self, command: Control) {
        let _ = self.controls.send(command);
    }
    pub fn receive(&self) -> (Status, Option<ColorImage>) {
        let mut output = self
            .output
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        (output.status.clone(), output.frame.take())
    }
}
impl Drop for Player {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

#[repr(C)]
struct Param {
    kind: c_int,
    data: *mut c_void,
}
#[repr(C)]
struct Event {
    id: c_int,
    error: c_int,
    userdata: u64,
    data: *mut c_void,
}
#[repr(C)]
struct Property {
    name: *const c_char,
    format: c_int,
    data: *mut c_void,
}
#[repr(C)]
struct EndFile {
    reason: c_int,
    error: c_int,
}
type Handle = *mut c_void;
type Callback = Option<unsafe extern "C" fn(*mut c_void)>;
struct Api {
    create: unsafe extern "C" fn() -> Handle,
    initialize: unsafe extern "C" fn(Handle) -> c_int,
    option: unsafe extern "C" fn(Handle, *const c_char, *const c_char) -> c_int,
    command: unsafe extern "C" fn(Handle, u64, *const *const c_char) -> c_int,
    observe: unsafe extern "C" fn(Handle, u64, *const c_char, c_int) -> c_int,
    event: unsafe extern "C" fn(Handle, f64) -> *const Event,
    error: unsafe extern "C" fn(c_int) -> *const c_char,
    destroy: unsafe extern "C" fn(Handle),
    render_create: unsafe extern "C" fn(*mut Handle, Handle, *mut Param) -> c_int,
    render_callback: unsafe extern "C" fn(Handle, Callback, *mut c_void),
    render_update: unsafe extern "C" fn(Handle) -> u64,
    render: unsafe extern "C" fn(Handle, *mut Param) -> c_int,
    render_free: unsafe extern "C" fn(Handle),
    _library: Library,
}
impl Api {
    fn load() -> Result<Self> {
        let paths = if cfg!(target_os = "macos") {
            vec![
                "/opt/homebrew/lib/libmpv.2.dylib",
                "/usr/local/lib/libmpv.2.dylib",
                "libmpv.2.dylib",
            ]
        } else {
            vec!["libmpv.so.2", "libmpv.so.1", "libmpv.so"]
        };
        let mut library = None;
        for path in paths {
            // SAFETY: symbols are the documented stable libmpv C API; the
            // library is retained for longer than every copied function pointer.
            if let Ok(candidate) = unsafe { Library::new(path) } {
                library = Some(candidate);
                break;
            }
        }
        let library = library.context("Embedded video needs libmpv. macOS: brew install mpv. Linux: install libmpv2 (or libmpv1)")?;
        unsafe {
            Ok(Self {
                create: *library.get(b"mpv_create\0")?,
                initialize: *library.get(b"mpv_initialize\0")?,
                option: *library.get(b"mpv_set_option_string\0")?,
                command: *library.get(b"mpv_command_async\0")?,
                observe: *library.get(b"mpv_observe_property\0")?,
                event: *library.get(b"mpv_wait_event\0")?,
                error: *library.get(b"mpv_error_string\0")?,
                destroy: *library.get(b"mpv_terminate_destroy\0")?,
                render_create: *library.get(b"mpv_render_context_create\0")?,
                render_callback: *library.get(b"mpv_render_context_set_update_callback\0")?,
                render_update: *library.get(b"mpv_render_context_update\0")?,
                render: *library.get(b"mpv_render_context_render\0")?,
                render_free: *library.get(b"mpv_render_context_free\0")?,
                _library: library,
            })
        }
    }
    fn check(&self, result: c_int) -> Result<()> {
        if result < 0 {
            // SAFETY: libmpv returns a static NUL-terminated description.
            anyhow::bail!(
                "{}",
                unsafe { CStr::from_ptr((self.error)(result)) }.to_string_lossy()
            );
        }
        Ok(())
    }
}
struct Engine {
    api: Api,
    client: Handle,
    render: Handle,
    dirty: Box<AtomicBool>,
}
unsafe extern "C" fn wake(data: *mut c_void) {
    // SAFETY: Engine keeps this allocation alive until the render context is
    // freed, which also unregisters and drains its callback.
    unsafe { &*data.cast::<AtomicBool>() }.store(true, Ordering::Relaxed);
}
impl Engine {
    fn new() -> Result<Self> {
        let api = Api::load()?;
        // SAFETY: the returned handle is owned by this worker and freed in Drop.
        let client = unsafe { (api.create)() };
        ensure!(!client.is_null(), "Cannot create embedded video player");
        let mut engine = Self {
            api,
            client,
            render: ptr::null_mut(),
            dirty: Box::new(AtomicBool::new(true)),
        };
        for (name, value) in [
            ("config", "no"),
            ("terminal", "no"),
            ("vo", "libmpv"),
            ("hwdec", "no"),
            ("pause", "yes"),
            ("keep-open", "yes"),
            ("volume", "80"),
            ("load-scripts", "no"),
            ("ytdl", "no"),
        ] {
            let (name, value) = (CString::new(name)?, CString::new(value)?);
            engine
                .api
                .check(unsafe { (engine.api.option)(client, name.as_ptr(), value.as_ptr()) })?;
        }
        if std::env::var_os("FEEDR_TEST_NULL_AUDIO").is_some() {
            engine
                .api
                .check(unsafe { (engine.api.option)(client, c"ao".as_ptr(), c"null".as_ptr()) })?;
        }
        engine
            .api
            .check(unsafe { (engine.api.initialize)(client) })?;
        for (id, name, format) in [
            (1, "time-pos", 5),
            (2, "duration", 5),
            (3, "pause", 3),
            (4, "volume", 5),
            (5, "dwidth", 4),
            (6, "dheight", 4),
            (7, "aid", 4),
        ] {
            let name = CString::new(name)?;
            engine
                .api
                .check(unsafe { (engine.api.observe)(client, id, name.as_ptr(), format) })?;
        }
        let mut advanced: c_int = 1;
        let mut params = [
            Param {
                kind: 1,
                data: c"sw".as_ptr().cast_mut().cast(),
            },
            Param {
                kind: 10,
                data: (&mut advanced as *mut c_int).cast(),
            },
            Param {
                kind: 0,
                data: ptr::null_mut(),
            },
        ];
        engine.api.check(unsafe {
            (engine.api.render_create)(&mut engine.render, client, params.as_mut_ptr())
        })?;
        unsafe {
            (engine.api.render_callback)(
                engine.render,
                Some(wake),
                (&*engine.dirty as *const AtomicBool).cast_mut().cast(),
            )
        };
        Ok(engine)
    }
    fn command(&self, args: &[&str]) -> Result<()> {
        let strings = args
            .iter()
            .map(|arg| CString::new(*arg))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let mut pointers: Vec<_> = strings.iter().map(|s| s.as_ptr()).collect();
        pointers.push(ptr::null());
        self.api
            .check(unsafe { (self.api.command)(self.client, 0, pointers.as_ptr()) })
    }
    fn frame(&self, width: usize, height: usize) -> Result<ColorImage> {
        // u32 storage guarantees libmpv's minimum four-byte pixel alignment.
        let mut pixels = vec![0u32; width * height];
        let mut size = [width as c_int, height as c_int];
        let mut stride = width * 4;
        let mut params = [
            Param {
                kind: 17,
                data: size.as_mut_ptr().cast(),
            },
            Param {
                kind: 18,
                data: c"rgb0".as_ptr().cast_mut().cast(),
            },
            Param {
                kind: 19,
                data: (&mut stride as *mut usize).cast(),
            },
            Param {
                kind: 20,
                data: pixels.as_mut_ptr().cast(),
            },
            Param {
                kind: 0,
                data: ptr::null_mut(),
            },
        ];
        self.api
            .check(unsafe { (self.api.render)(self.render, params.as_mut_ptr()) })?;
        let mut rgba = Vec::with_capacity(width * height * 4);
        for pixel in pixels {
            let bytes = pixel.to_ne_bytes();
            rgba.extend_from_slice(&[bytes[0], bytes[1], bytes[2], 255]);
        }
        Ok(ColorImage::from_rgba_unmultiplied([width, height], &rgba))
    }
}
impl Drop for Engine {
    fn drop(&mut self) {
        // Render context must be freed before its parent client and library.
        unsafe {
            if !self.render.is_null() {
                (self.api.render_free)(self.render);
            }
            (self.api.destroy)(self.client);
        }
    }
}
fn play(
    path: &Path,
    controls: &mpsc::Receiver<Control>,
    output: &Mutex<Output>,
    stop: &AtomicBool,
    ctx: &egui::Context,
) -> Result<()> {
    ensure!(
        path.is_absolute() && path.is_file(),
        "Video resource is unavailable or unresolved"
    );
    let engine = Engine::new()?;
    engine.command(&[
        "loadfile",
        path.to_str().context("Video path is not UTF-8")?,
    ])?;
    let mut status = Status::default();
    let (mut width, mut height) = (960usize, 540usize);
    let mut last_status = Instant::now();
    while !stop.load(Ordering::Relaxed) {
        for control in controls.try_iter() {
            match control {
                Control::Pause(paused) => {
                    engine.command(&["set", "pause", if paused { "yes" } else { "no" }])?
                }
                Control::Seek(seconds) => {
                    engine.command(&["seek", &seconds.max(0.).to_string(), "absolute+exact"])?
                }
                Control::Volume(volume) => {
                    engine.command(&["set", "volume", &volume.clamp(0., 100.).to_string()])?
                }
            }
        }
        // Copy observed values before the next wait_event invalidates pointers.
        loop {
            let event = unsafe { &*(engine.api.event)(engine.client, 0.) };
            if event.id == 0 {
                break;
            }
            if event.error < 0 {
                engine.api.check(event.error)?;
            }
            if event.id == 7 && !event.data.is_null() {
                let end = unsafe { &*event.data.cast::<EndFile>() };
                if end.error < 0 {
                    engine.api.check(end.error)?;
                }
            }
            if event.id == 22 && !event.data.is_null() {
                let property = unsafe { &*event.data.cast::<Property>() };
                if property.data.is_null() {
                    continue;
                }
                match (event.userdata, property.format) {
                    (1, 5) => status.position = unsafe { *property.data.cast::<f64>() },
                    (2, 5) => status.duration = unsafe { *property.data.cast::<f64>() },
                    (3, 3) => status.paused = unsafe { *property.data.cast::<c_int>() != 0 },
                    (4, 5) => status.volume = unsafe { *property.data.cast::<f64>() },
                    (5, 4) => {
                        width = unsafe { *property.data.cast::<i64>() }.clamp(1, 16384) as usize
                    }
                    (6, 4) => {
                        height = unsafe { *property.data.cast::<i64>() }.clamp(1, 16384) as usize
                    }
                    (7, 4) => status.audio = unsafe { *property.data.cast::<i64>() > 0 },
                    _ => (),
                }
            }
        }
        let mut frame = None;
        if engine.dirty.swap(false, Ordering::Relaxed)
            && unsafe { (engine.api.render_update)(engine.render) } & 1 != 0
        {
            let scale = (1280. / width as f64).min(720. / height as f64).min(1.);
            frame = Some(engine.frame(
                (width as f64 * scale).max(1.) as usize,
                (height as f64 * scale).max(1.) as usize,
            )?);
        }
        if frame.is_some() || last_status.elapsed() > Duration::from_millis(100) {
            let mut output = output.lock().unwrap_or_else(|error| error.into_inner());
            output.status = status.clone();
            if frame.is_some() {
                output.frame = frame;
            }
            last_status = Instant::now();
            ctx.request_repaint();
        }
        thread::sleep(Duration::from_millis(8));
    }
    Ok(())
}
