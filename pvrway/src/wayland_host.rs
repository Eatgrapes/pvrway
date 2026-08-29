use std::fs::File;
use std::os::unix::fs::FileExt;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::thread;
use std::time::Duration;

use wayland_protocols::xdg::shell::server::{xdg_surface, xdg_toplevel, xdg_wm_base};
use wayland_server::backend::ClientData;
use wayland_server::backend::GlobalId;
use wayland_server::protocol::{
    wl_buffer, wl_compositor, wl_output, wl_region, wl_shm, wl_shm_pool, wl_surface,
};
use wayland_server::{
    DataInit, Dispatch, Display, DisplayHandle, GlobalDispatch, ListeningSocket, New, Resource,
};

pub struct State {
    globals: Vec<GlobalId>,
}

struct ShmPool {
    file: File,
}

struct ShmBuffer {
    pool: Arc<ShmPool>,
    offset: u64,
    width: i32,
    height: i32,
    stride: i32,
}

#[derive(Default)]
struct SurfaceState {
    pending_buffer: Mutex<Option<Arc<ShmBuffer>>>,
}

impl GlobalDispatch<wl_compositor::WlCompositor, ()> for State {
    fn bind(
        _state: &mut Self,
        _handle: &DisplayHandle,
        _client: &wayland_server::Client,
        resource: New<wl_compositor::WlCompositor>,
        _global_data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }
}

impl Dispatch<wl_compositor::WlCompositor, ()> for State {
    fn request(
        _state: &mut Self,
        _client: &wayland_server::Client,
        _resource: &wl_compositor::WlCompositor,
        request: wl_compositor::Request,
        _data: &(),
        _handle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            wl_compositor::Request::CreateSurface { id } => {
                data_init.init::<wl_surface::WlSurface, _>(id, Arc::new(SurfaceState::default()));
            }
            wl_compositor::Request::CreateRegion { id } => {
                data_init.init::<wl_region::WlRegion, _>(id, ());
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_surface::WlSurface, Arc<SurfaceState>> for State {
    fn request(
        _state: &mut Self,
        _client: &wayland_server::Client,
        _resource: &wl_surface::WlSurface,
        request: wl_surface::Request,
        data: &Arc<SurfaceState>,
        _handle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            wl_surface::Request::Attach { buffer, .. } => {
                let buffer = buffer.and_then(|buffer| buffer.data::<Arc<ShmBuffer>>().cloned());
                *data.pending_buffer.lock().expect("surface buffer lock") = buffer;
            }
            wl_surface::Request::Commit => {
                if let Some(buffer) = data
                    .pending_buffer
                    .lock()
                    .expect("surface buffer lock")
                    .as_ref()
                {
                    let mut pixel = [0_u8; 4];
                    match buffer.pool.file.read_at(&mut pixel, buffer.offset) {
                        Ok(4) => log::info!(
                            "shared buffer committed: {}x{} stride={} first_pixel={pixel:02x?}",
                            buffer.width,
                            buffer.height,
                            buffer.stride
                        ),
                        Ok(bytes) => log::warn!("short shared buffer read: {bytes} bytes"),
                        Err(error) => log::warn!("read shared buffer: {error}"),
                    }
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_region::WlRegion, ()> for State {
    fn request(
        _state: &mut Self,
        _client: &wayland_server::Client,
        _resource: &wl_region::WlRegion,
        _request: wl_region::Request,
        _data: &(),
        _handle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
    }
}

impl GlobalDispatch<wl_shm::WlShm, ()> for State {
    fn bind(
        _state: &mut Self,
        _handle: &DisplayHandle,
        _client: &wayland_server::Client,
        resource: New<wl_shm::WlShm>,
        _global_data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        let resource = data_init.init(resource, ());
        resource.format(wl_shm::Format::Argb8888);
        resource.format(wl_shm::Format::Xrgb8888);
    }
}

impl Dispatch<wl_shm::WlShm, ()> for State {
    fn request(
        _state: &mut Self,
        _client: &wayland_server::Client,
        _resource: &wl_shm::WlShm,
        request: wl_shm::Request,
        _data: &(),
        _handle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        if let wl_shm::Request::CreatePool { id, fd, .. } = request {
            data_init.init::<wl_shm_pool::WlShmPool, _>(
                id,
                Arc::new(ShmPool {
                    file: File::from(fd),
                }),
            );
        }
    }
}

impl Dispatch<wl_shm_pool::WlShmPool, Arc<ShmPool>> for State {
    fn request(
        _state: &mut Self,
        _client: &wayland_server::Client,
        _resource: &wl_shm_pool::WlShmPool,
        request: wl_shm_pool::Request,
        data: &Arc<ShmPool>,
        _handle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        if let wl_shm_pool::Request::CreateBuffer {
            id,
            offset,
            width,
            height,
            stride,
            ..
        } = request
        {
            data_init.init::<wl_buffer::WlBuffer, _>(
                id,
                Arc::new(ShmBuffer {
                    pool: data.clone(),
                    offset: offset as u64,
                    width,
                    height,
                    stride,
                }),
            );
        }
    }
}

impl Dispatch<wl_buffer::WlBuffer, Arc<ShmBuffer>> for State {
    fn request(
        _state: &mut Self,
        _client: &wayland_server::Client,
        _resource: &wl_buffer::WlBuffer,
        _request: wl_buffer::Request,
        _data: &Arc<ShmBuffer>,
        _handle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
    }
}

impl GlobalDispatch<wl_output::WlOutput, ()> for State {
    fn bind(
        _state: &mut Self,
        _handle: &DisplayHandle,
        _client: &wayland_server::Client,
        resource: New<wl_output::WlOutput>,
        _global_data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        let resource = data_init.init(resource, ());
        resource.geometry(
            0,
            0,
            160,
            72,
            wl_output::Subpixel::Unknown,
            "PvrWay".to_string(),
            "Redmi 9A".to_string(),
            wl_output::Transform::Normal,
        );
        resource.mode(
            wl_output::Mode::Current | wl_output::Mode::Preferred,
            1600,
            720,
            60_000,
        );
        resource.scale(1);
        resource.done();
    }
}

impl Dispatch<wl_output::WlOutput, ()> for State {
    fn request(
        _state: &mut Self,
        _client: &wayland_server::Client,
        _resource: &wl_output::WlOutput,
        _request: wl_output::Request,
        _data: &(),
        _handle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
    }
}

impl GlobalDispatch<xdg_wm_base::XdgWmBase, ()> for State {
    fn bind(
        _state: &mut Self,
        _handle: &DisplayHandle,
        _client: &wayland_server::Client,
        resource: New<xdg_wm_base::XdgWmBase>,
        _global_data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }
}

impl Dispatch<xdg_wm_base::XdgWmBase, ()> for State {
    fn request(
        _state: &mut Self,
        _client: &wayland_server::Client,
        _resource: &xdg_wm_base::XdgWmBase,
        request: xdg_wm_base::Request,
        _data: &(),
        _handle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            xdg_wm_base::Request::GetXdgSurface { id, .. } => {
                data_init.init::<xdg_surface::XdgSurface, _>(id, ());
            }
            xdg_wm_base::Request::Pong { serial } => {
                log::trace!("xdg_wm_base pong for serial {serial}");
            }
            _ => {}
        }
    }
}

impl Dispatch<xdg_surface::XdgSurface, ()> for State {
    fn request(
        _state: &mut Self,
        _client: &wayland_server::Client,
        resource: &xdg_surface::XdgSurface,
        request: xdg_surface::Request,
        _data: &(),
        _handle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        if let xdg_surface::Request::GetToplevel { id } = request {
            let toplevel = data_init.init::<xdg_toplevel::XdgToplevel, _>(id, ());
            toplevel.configure(720, 1504, Vec::new());
            resource.configure(1);
        }
    }
}

impl Dispatch<xdg_toplevel::XdgToplevel, ()> for State {
    fn request(
        _state: &mut Self,
        _client: &wayland_server::Client,
        _resource: &xdg_toplevel::XdgToplevel,
        _request: xdg_toplevel::Request,
        _data: &(),
        _handle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
    }
}

pub fn spawn() -> Result<(), String> {
    let socket = ListeningSocket::bind_absolute(PathBuf::from(
        "/data/user/0/io.eatgrapes.pvrway/files/pvrway.sock",
    ))
    .map_err(|error| format!("bind Wayland socket: {error:?}"))?;
    thread::Builder::new()
        .name("pvrway-wayland".to_string())
        .spawn(move || run(socket))
        .map_err(|error| format!("spawn Wayland server: {error}"))?;
    Ok(())
}

fn run(socket: ListeningSocket) {
    let mut display = match Display::<State>::new() {
        Ok(display) => display,
        Err(error) => {
            log::error!("create Wayland display: {error:?}");
            return;
        }
    };
    let mut state = State {
        globals: Vec::new(),
    };
    state.globals.push(
        display
            .handle()
            .create_global::<State, wl_compositor::WlCompositor, ()>(5, ()),
    );
    state.globals.push(
        display
            .handle()
            .create_global::<State, xdg_wm_base::XdgWmBase, ()>(6, ()),
    );
    state.globals.push(
        display
            .handle()
            .create_global::<State, wl_shm::WlShm, ()>(1, ()),
    );
    state.globals.push(
        display
            .handle()
            .create_global::<State, wl_output::WlOutput, ()>(3, ()),
    );
    let client_data: Arc<dyn ClientData> = Arc::new(());

    loop {
        while let Ok(Some(stream)) = socket.accept() {
            if let Err(error) = display.handle().insert_client(stream, client_data.clone()) {
                log::warn!("accept Wayland client: {error}");
            }
        }
        if let Err(error) = display.dispatch_clients(&mut state) {
            log::warn!("dispatch Wayland clients: {error}");
        }
        if let Err(error) = display.flush_clients() {
            log::warn!("flush Wayland clients: {error}");
        }
        thread::sleep(Duration::from_millis(4));
    }
}
