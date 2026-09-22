// just enough egl for one opengl es 2 context and a window surface. libEGL.so has been there since android 2.3

use std::ffi::{c_char, c_void, CStr};
use std::ptr::null_mut;

type Display = *mut c_void;
type Config = *mut c_void;
type Context = *mut c_void;
pub type Surface = *mut c_void;

const NONE: i32 = 0x3038;
const RED_SIZE: i32 = 0x3024;
const GREEN_SIZE: i32 = 0x3023;
const BLUE_SIZE: i32 = 0x3022;
const RENDERABLE_TYPE: i32 = 0x3040;
const OPENGL_ES2_BIT: i32 = 0x0004;
const SURFACE_TYPE: i32 = 0x3033;
const WINDOW_BIT: i32 = 0x0004;
const NATIVE_VISUAL_ID: i32 = 0x302E;
const CONTEXT_CLIENT_VERSION: i32 = 0x3098;

#[link(name = "EGL")]
unsafe extern "C" {
    fn eglGetDisplay(native: *mut c_void) -> Display;
    fn eglInitialize(display: Display, major: *mut i32, minor: *mut i32) -> u32;
    fn eglChooseConfig(display: Display, attribs: *const i32, configs: *mut Config, size: i32, count: *mut i32) -> u32;
    fn eglGetConfigAttrib(display: Display, config: Config, attribute: i32, value: *mut i32) -> u32;
    fn eglCreateContext(display: Display, config: Config, share: Context, attribs: *const i32) -> Context;
    fn eglCreateWindowSurface(display: Display, config: Config, window: *mut c_void, attribs: *const i32) -> Surface;
    fn eglDestroySurface(display: Display, surface: Surface) -> u32;
    fn eglMakeCurrent(display: Display, draw: Surface, read: Surface, context: Context) -> u32;
    fn eglSwapBuffers(display: Display, surface: Surface) -> u32;
    fn eglGetProcAddress(name: *const c_char) -> *mut c_void;
    fn eglGetError() -> i32;
}

#[link(name = "android")]
unsafe extern "C" {
    fn ANativeWindow_setBuffersGeometry(window: *mut c_void, width: i32, height: i32, format: i32) -> i32;
}

fn error(what: &str) -> String {
    format!("{what} failed, egl error {:#x}", unsafe { eglGetError() })
}

/// display, config and context: made once, outlive the window (android takes that away whenever the app goes to the background)
pub struct Egl {
    display: Display,
    config: Config,
    context: Context,
}

impl Egl {
    pub fn new() -> Result<Egl, String> {
        unsafe {
            let display = eglGetDisplay(null_mut());
            if display.is_null() || eglInitialize(display, null_mut(), null_mut()) == 0 {
                return Err(error("eglInitialize"));
            }
            // plain rgb888, no depth: egui draws flat triangles back to front
            let attribs = [
                RENDERABLE_TYPE, OPENGL_ES2_BIT, SURFACE_TYPE, WINDOW_BIT,
                RED_SIZE, 8, GREEN_SIZE, 8, BLUE_SIZE, 8, NONE,
            ];
            let (mut config, mut count) = (null_mut(), 0);
            if eglChooseConfig(display, attribs.as_ptr(), &mut config, 1, &mut count) == 0 || count == 0 {
                return Err(error("eglChooseConfig"));
            }
            let context = eglCreateContext(display, config, null_mut(), [CONTEXT_CLIENT_VERSION, 2, NONE].as_ptr());
            if context.is_null() {
                return Err(error("eglCreateContext"));
            }
            Ok(Egl { display, config, context })
        }
    }

    /// surface for an ANativeWindow, current from here on
    pub fn attach(&self, window: *mut c_void) -> Result<Surface, String> {
        unsafe {
            // the window's buffers in the format the config renders in, or the colors come out wrong
            let mut format = 0;
            eglGetConfigAttrib(self.display, self.config, NATIVE_VISUAL_ID, &mut format);
            ANativeWindow_setBuffersGeometry(window, 0, 0, format);

            let surface = eglCreateWindowSurface(self.display, self.config, window, [NONE].as_ptr());
            if surface.is_null() {
                return Err(error("eglCreateWindowSurface"));
            }
            if eglMakeCurrent(self.display, surface, surface, self.context) == 0 {
                let e = error("eglMakeCurrent");
                eglDestroySurface(self.display, surface);
                return Err(e);
            }
            Ok(surface)
        }
    }

    /// before the window goes. without the surfaceless extension (which android 4 doesnt have) the context
    /// cant stay current without one, so it gets let go too
    pub fn detach(&self, surface: Surface) {
        unsafe {
            eglMakeCurrent(self.display, null_mut(), null_mut(), null_mut());
            eglDestroySurface(self.display, surface);
        }
    }

    pub fn swap(&self, surface: Surface) {
        unsafe { eglSwapBuffers(self.display, surface) };
    }

    /// the gl functions for glow. eglGetProcAddress is no good for them on android 4: it only knows extensions,
    /// hands out a stub for any name it gets (known or not) and runs out of stubs after 256. glow asks for
    /// a thousand. so the core ones come straight out of libGLESv2, and eglGetProcAddress only gets asked
    /// for the vertex array extension egui_glow uses when the driver has it. needs the context current
    pub fn gl(&self) -> Result<glow::Context, String> {
        const VAO: [&CStr; 4] =
            [c"glGenVertexArraysOES", c"glBindVertexArrayOES", c"glDeleteVertexArraysOES", c"glIsVertexArrayOES"];
        const GL_EXTENSIONS: u32 = 0x1F03;

        unsafe {
            let gles = libc::dlopen(c"libGLESv2.so".as_ptr(), libc::RTLD_NOW);
            if gles.is_null() {
                return Err("cant open libGLESv2.so".into());
            }
            let get_string = libc::dlsym(gles, c"glGetString".as_ptr());
            if get_string.is_null() {
                return Err("libGLESv2.so has no glGetString".into());
            }
            let get_string: unsafe extern "C" fn(u32) -> *const c_char = std::mem::transmute(get_string);
            let extensions = get_string(GL_EXTENSIONS);
            let vao = !extensions.is_null()
                && CStr::from_ptr(extensions).to_bytes().split(|b| *b == b' ').any(|e| e == b"GL_OES_vertex_array_object");

            Ok(glow::Context::from_loader_function_cstr(|name| {
                let f = libc::dlsym(gles, name.as_ptr());
                if !f.is_null() {
                    f
                } else if vao && VAO.contains(&name) {
                    eglGetProcAddress(name.as_ptr())
                } else {
                    null_mut()
                }
            }))
        }
    }
}
