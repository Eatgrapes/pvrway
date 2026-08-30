use std::cell::{Cell, RefCell};
use std::ffi::c_void;

use glow::HasContext;
use khronos_egl as egl;
use libloading::Library;
use ndk::native_window::NativeWindow;

use crate::frame_protocol::CommittedFrame;

const VERTEX_SHADER: &str = r#"
attribute vec2 position;
attribute vec2 tex_coord;
varying vec2 texture_position;

void main() {
    gl_Position = vec4(position, 0.0, 1.0);
    texture_position = tex_coord;
}
"#;

const FRAGMENT_SHADER: &str = r#"
precision mediump float;
varying vec2 texture_position;
uniform sampler2D frame_texture;

void main() {
    vec4 color = texture2D(frame_texture, texture_position);
    if (texture_position.x > 0.91 && texture_position.y > 0.88) {
        vec2 button = (texture_position - vec2(0.91, 0.88)) / vec2(0.09, 0.12);
        float edge = step(0.08, button.x) * step(0.08, button.y)
            * step(button.x, 0.92) * step(button.y, 0.92);
        color.rgb = mix(color.rgb, vec3(0.38, 0.16, 0.72), edge * 0.9);
        float row = step(0.30, button.y) * step(button.y, 0.38)
            + step(0.46, button.y) * step(button.y, 0.54)
            + step(0.62, button.y) * step(button.y, 0.70);
        color.rgb = mix(color.rgb, vec3(0.92, 0.86, 1.0), min(row, 1.0) * edge);
    }
    gl_FragColor = vec4(color.rgb, 1.0);
}
"#;

pub struct EglHost {
    egl: egl::DynamicInstance<egl::EGL1_4>,
    display: egl::Display,
    surface: egl::Surface,
    context: egl::Context,
    gl: glow::Context,
    program: glow::NativeProgram,
    texture: glow::NativeTexture,
    vertex_buffer: glow::NativeBuffer,
    position_location: u32,
    texture_location: u32,
    bgra_supported: Cell<bool>,
    frame_count: Cell<u64>,
    rgba_pixels: RefCell<Vec<u8>>,
    _gles_library: Library,
}

impl EglHost {
    pub fn new(window: &NativeWindow) -> Result<Self, String> {
        let library = unsafe {
            // SAFETY: Android provides libEGL in the application linker namespace.
            Library::new("libEGL.so")
        }
        .map_err(|error| format!("load libEGL: {error}"))?;
        let egl = unsafe {
            // SAFETY: The library is libEGL and EGL 1.4 is part of Android's native ABI.
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
        let context = egl
            .create_context(
                display,
                config,
                None,
                &[egl::CONTEXT_CLIENT_VERSION, 2, egl::NONE],
            )
            .map_err(|error| format!("create OpenGL ES context: {error:?}"))?;
        let surface = unsafe {
            // SAFETY: NativeWindow belongs to this Activity and remains alive for EglHost's lifetime.
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

        let gles_library = unsafe {
            // SAFETY: Android exposes the OpenGL ES 2 library to NativeActivity applications.
            Library::new("libGLESv2.so")
        }
        .map_err(|error| format!("load libGLESv2: {error}"))?;
        let gl = unsafe {
            // SAFETY: Each returned address comes from EGL or libGLESv2 for the current context.
            glow::Context::from_loader_function(|name| {
                egl.get_proc_address(name)
                    .map(|function| function as *const () as *const c_void)
                    .unwrap_or_else(|| {
                        let symbol = format!("{name}\0");
                        gles_library
                            .get::<unsafe extern "C" fn()>(symbol.as_bytes())
                            .map(|function| *function as *const () as *const c_void)
                            .unwrap_or(std::ptr::null())
                    })
            })
        };
        let bgra_supported = gl
            .supported_extensions()
            .iter()
            .any(|extension| extension == "GL_EXT_texture_format_BGRA8888");
        log::info!("PowerVR BGRA texture upload: {bgra_supported}");
        let program = create_program(&gl)?;
        let texture = unsafe {
            // SAFETY: The EGL context is current and the handle remains owned by EglHost.
            gl.create_texture()
                .map_err(|error| format!("create texture: {error}"))?
        };
        let vertex_buffer = unsafe {
            // SAFETY: The EGL context is current and the handle remains owned by EglHost.
            gl.create_buffer()
                .map_err(|error| format!("create vertex buffer: {error}"))?
        };
        let position_location = unsafe {
            // SAFETY: program is linked and owned by this context.
            gl.get_attrib_location(program, "position")
                .ok_or_else(|| "missing position attribute".to_string())?
        };
        let texture_location = unsafe {
            // SAFETY: program is linked and owned by this context.
            gl.get_attrib_location(program, "tex_coord")
                .ok_or_else(|| "missing texture coordinate attribute".to_string())?
        };
        let vertices: [f32; 16] = [
            -1.0, -1.0, 0.0, 1.0, 1.0, -1.0, 1.0, 1.0, -1.0, 1.0, 0.0, 0.0, 1.0, 1.0, 1.0, 0.0,
        ];
        let vertex_bytes = unsafe {
            // SAFETY: f32 has no padding and the slice covers exactly the vertices array.
            std::slice::from_raw_parts(
                vertices.as_ptr().cast::<u8>(),
                vertices.len() * std::mem::size_of::<f32>(),
            )
        };
        unsafe {
            // SAFETY: All GL handles were created by this current context.
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(vertex_buffer));
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, vertex_bytes, glow::STATIC_DRAW);
            gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MIN_FILTER,
                glow::LINEAR as i32,
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MAG_FILTER,
                glow::LINEAR as i32,
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_WRAP_S,
                glow::CLAMP_TO_EDGE as i32,
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_WRAP_T,
                glow::CLAMP_TO_EDGE as i32,
            );
            gl.clear_color(0.09, 0.05, 0.17, 1.0);
            gl.clear(glow::COLOR_BUFFER_BIT);
        }
        egl.swap_buffers(display, surface)
            .map_err(|error| format!("present EGL frame: {error:?}"))?;

        Ok(Self {
            egl,
            display,
            surface,
            context,
            gl,
            program,
            texture,
            vertex_buffer,
            position_location,
            texture_location,
            bgra_supported: Cell::new(bgra_supported),
            frame_count: Cell::new(0),
            rgba_pixels: RefCell::new(Vec::new()),
            _gles_library: gles_library,
        })
    }

    pub fn present_wayland_frame(&self, frame: &CommittedFrame) -> Result<(), String> {
        let width = frame.width as usize;
        let height = frame.height as usize;
        let source_stride = frame.stride as usize;
        let row_bytes = width
            .checked_mul(4)
            .ok_or_else(|| "frame width overflow".to_string())?;
        if source_stride < row_bytes || frame.pixels.len() < source_stride * height {
            return Err("invalid frame stride".to_string());
        }
        let direct_bgra = self.bgra_supported.get() && source_stride == row_bytes;
        let mut rgba = self.rgba_pixels.borrow_mut();
        if !direct_bgra {
            rgba.resize(row_bytes * height, 0);
            for row in 0..height {
                let source = &frame.pixels[row * source_stride..row * source_stride + row_bytes];
                let destination = &mut rgba[row * row_bytes..(row + 1) * row_bytes];
                for (bgra, rgba) in source.chunks_exact(4).zip(destination.chunks_exact_mut(4)) {
                    rgba[0] = bgra[2];
                    rgba[1] = bgra[1];
                    rgba[2] = bgra[0];
                    rgba[3] = 255;
                }
            }
        }
        let (texture_format, texture_internal, pixels) = if direct_bgra {
            (glow::BGRA, glow::BGRA as i32, frame.pixels.as_slice())
        } else {
            (glow::RGBA, glow::RGBA as i32, rgba.as_slice())
        };
        unsafe {
            // SAFETY: The EGL context remains current and all handles belong to it.
            self.gl
                .viewport(0, 0, frame.width as i32, frame.height as i32);
            self.gl.clear_color(0.0, 0.0, 0.0, 1.0);
            self.gl.clear(glow::COLOR_BUFFER_BIT);
            self.gl.use_program(Some(self.program));
            self.gl
                .bind_buffer(glow::ARRAY_BUFFER, Some(self.vertex_buffer));
            self.gl.enable_vertex_attrib_array(self.position_location);
            self.gl
                .vertex_attrib_pointer_f32(self.position_location, 2, glow::FLOAT, false, 16, 0);
            self.gl.enable_vertex_attrib_array(self.texture_location);
            self.gl
                .vertex_attrib_pointer_f32(self.texture_location, 2, glow::FLOAT, false, 16, 8);
            self.gl.active_texture(glow::TEXTURE0);
            self.gl.bind_texture(glow::TEXTURE_2D, Some(self.texture));
            self.gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
            self.gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                texture_internal,
                frame.width as i32,
                frame.height as i32,
                0,
                texture_format,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(Some(pixels)),
            );
            if let Some(location) = self.gl.get_uniform_location(self.program, "frame_texture") {
                self.gl.uniform_1_i32(Some(&location), 0);
            }
            self.gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
        }
        self.egl
            .swap_buffers(self.display, self.surface)
            .map_err(|error| format!("present Wayland texture: {error:?}"))?;
        let frame_count = self.frame_count.get() + 1;
        self.frame_count.set(frame_count);
        if frame_count == 1 || frame_count % 60 == 0 {
            log::info!(
                "rendered {} proxy frames: {}x{} stride={} bytes={}",
                frame_count,
                frame.width,
                frame.height,
                frame.stride,
                frame.pixels.len()
            );
        }
        Ok(())
    }
}

impl Drop for EglHost {
    fn drop(&mut self) {
        unsafe {
            // SAFETY: The context is current while its owned GL objects are deleted.
            self.gl.delete_buffer(self.vertex_buffer);
            self.gl.delete_texture(self.texture);
            self.gl.delete_program(self.program);
        }
        let _ = self.egl.make_current(self.display, None, None, None);
        let _ = self.egl.destroy_surface(self.display, self.surface);
        let _ = self.egl.destroy_context(self.display, self.context);
        let _ = self.egl.terminate(self.display);
    }
}

fn create_program(gl: &glow::Context) -> Result<glow::NativeProgram, String> {
    unsafe {
        // SAFETY: Shader and program handles are created, checked, and destroyed within this context.
        let program = gl
            .create_program()
            .map_err(|error| format!("create program: {error}"))?;
        let vertex = compile_shader(gl, glow::VERTEX_SHADER, VERTEX_SHADER)?;
        let fragment = compile_shader(gl, glow::FRAGMENT_SHADER, FRAGMENT_SHADER)?;
        gl.attach_shader(program, vertex);
        gl.attach_shader(program, fragment);
        gl.link_program(program);
        gl.detach_shader(program, vertex);
        gl.detach_shader(program, fragment);
        gl.delete_shader(vertex);
        gl.delete_shader(fragment);
        if !gl.get_program_link_status(program) {
            let error = gl.get_program_info_log(program);
            gl.delete_program(program);
            return Err(format!("link frame program: {error}"));
        }
        Ok(program)
    }
}

fn compile_shader(
    gl: &glow::Context,
    shader_type: u32,
    source: &str,
) -> Result<glow::NativeShader, String> {
    unsafe {
        // SAFETY: The shader is owned by this context and deleted on compilation failure.
        let shader = gl
            .create_shader(shader_type)
            .map_err(|error| format!("create shader: {error}"))?;
        gl.shader_source(shader, source);
        gl.compile_shader(shader);
        if !gl.get_shader_compile_status(shader) {
            let error = gl.get_shader_info_log(shader);
            gl.delete_shader(shader);
            return Err(format!("compile frame shader: {error}"));
        }
        Ok(shader)
    }
}
