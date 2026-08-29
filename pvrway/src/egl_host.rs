use std::ffi::c_void;

use khronos_egl as egl;
use libloading::Library;
use ndk::native_window::NativeWindow;

const GL_COLOR_BUFFER_BIT: u32 = 0x0000_4000;

pub struct EglHost {
    egl: egl::DynamicInstance<egl::EGL1_4>,
    display: egl::Display,
    surface: egl::Surface,
    context: egl::Context,
}

impl EglHost {
    pub fn new(window: &NativeWindow) -> Result<Self, String> {
        let library = unsafe {
            // SAFETY: Android provides libEGL in the application linker namespace.
            Library::new("libEGL.so")
        }
        .map_err(|error| format!("load libEGL: {error}"))?;
        let egl = unsafe {
            // SAFETY: The library is libEGL and the requested EGL 1.4 symbols are part of Android's EGL ABI.
            egl::DynamicInstance::<egl::EGL1_4>::load_required_from(library)
        }
        .map_err(|error| format!("load EGL entry points: {error}"))?;
        let display = unsafe {
            // SAFETY: EGL_DEFAULT_DISPLAY is the Android process display connection.
            egl.get_display(egl::DEFAULT_DISPLAY)
        }
        .ok_or_else(|| "get EGL display returned no display".to_string())?;
        egl.initialize(display)
            .map_err(|error| format!("initialize EGL: {error:?}"))?;
        egl.bind_api(egl::OPENGL_ES_API)
            .map_err(|error| format!("bind OpenGL ES: {error:?}"))?;

        let config_attributes = [
            egl::SURFACE_TYPE,
            egl::WINDOW_BIT,
            egl::RENDERABLE_TYPE,
            egl::OPENGL_ES2_BIT,
            egl::RED_SIZE,
            8,
            egl::GREEN_SIZE,
            8,
            egl::BLUE_SIZE,
            8,
            egl::ALPHA_SIZE,
            8,
            egl::NONE,
        ];
        let config = egl
            .choose_first_config(display, &config_attributes)
            .map_err(|error| format!("choose EGL config: {error:?}"))?
            .ok_or_else(|| "no RGBA OpenGL ES window config".to_string())?;
        let context_attributes = [egl::CONTEXT_CLIENT_VERSION, 2, egl::NONE];
        let context = egl
            .create_context(display, config, None, &context_attributes)
            .map_err(|error| format!("create OpenGL ES context: {error:?}"))?;
        let surface = unsafe {
            // SAFETY: NativeWindow belongs to this Android Activity and remains alive for EglHost's lifetime.
            egl.create_window_surface(
                display,
                config,
                window.ptr().as_ptr().cast::<c_void>(),
                None,
            )
        }
        .map_err(|error| format!("create EGL window surface: {error:?}"))?;
        egl.make_current(display, Some(surface), Some(surface), Some(context))
            .map_err(|error| format!("make EGL context current: {error:?}"))?;
        egl.swap_interval(display, 1)
            .map_err(|error| format!("set swap interval: {error:?}"))?;

        let gles = unsafe {
            // SAFETY: Android exposes the OpenGL ES 2 ABI to NativeActivity applications.
            Library::new("libGLESv2.so")
        }
        .map_err(|error| format!("load libGLESv2: {error}"))?;
        unsafe {
            // SAFETY: Both symbols use the published OpenGL ES 2 ABI and are invoked while the context is current.
            let clear_color = gles
                .get::<unsafe extern "C" fn(f32, f32, f32, f32)>(b"glClearColor\0")
                .map_err(|error| format!("load glClearColor: {error}"))?;
            let clear = gles
                .get::<unsafe extern "C" fn(u32)>(b"glClear\0")
                .map_err(|error| format!("load glClear: {error}"))?;
            clear_color(0.09, 0.05, 0.17, 1.0);
            clear(GL_COLOR_BUFFER_BIT);
        }
        egl.swap_buffers(display, surface)
            .map_err(|error| format!("present EGL frame: {error:?}"))?;

        Ok(Self {
            egl,
            display,
            surface,
            context,
        })
    }

    pub fn present_wayland_frame(&self) -> Result<(), String> {
        let gles = unsafe {
            // SAFETY: Android exposes the OpenGL ES 2 ABI to NativeActivity applications.
            Library::new("libGLESv2.so")
        }
        .map_err(|error| format!("load libGLESv2: {error}"))?;
        unsafe {
            // SAFETY: The EGL context belongs to the Android main thread and remains current while the window lives.
            let clear_color = gles
                .get::<unsafe extern "C" fn(f32, f32, f32, f32)>(b"glClearColor\0")
                .map_err(|error| format!("load glClearColor: {error}"))?;
            let clear = gles
                .get::<unsafe extern "C" fn(u32)>(b"glClear\0")
                .map_err(|error| format!("load glClear: {error}"))?;
            clear_color(0.20, 0.06, 0.36, 1.0);
            clear(GL_COLOR_BUFFER_BIT);
        }
        self.egl
            .swap_buffers(self.display, self.surface)
            .map_err(|error| format!("present Wayland frame: {error:?}"))
    }
}

impl Drop for EglHost {
    fn drop(&mut self) {
        let _ = self.egl.make_current(self.display, None, None, None);
        let _ = self.egl.destroy_surface(self.display, self.surface);
        let _ = self.egl.destroy_context(self.display, self.context);
        let _ = self.egl.terminate(self.display);
    }
}
