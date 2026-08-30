use std::fs::{self, File};
use std::os::unix::fs::FileExt;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::thread;
use std::time::Duration;

use wayland_protocols::xdg::shell::server::{xdg_surface, xdg_toplevel, xdg_wm_base};
use wayland_server::backend::{ClientData, ClientId, DisconnectReason, GlobalId};
use wayland_server::protocol::{
    wl_buffer, wl_callback, wl_compositor, wl_output, wl_pointer, wl_region, wl_seat, wl_shm,
    wl_shm_pool, wl_surface,
};
use wayland_server::{
    DataInit, Dispatch, Display, DisplayHandle, GlobalDispatch, ListeningSocket, New, Resource,
};

use crate::frame_protocol::CommittedFrame;
use crate::input_protocol::{PROXY_INPUT_SOCKET, PointerAction, PointerPacket, read_pointer};

pub struct State {
    globals: Vec<GlobalId>,
    frame_tx: SyncSender<CommittedFrame>,
    input_rx: Receiver<PointerPacket>,
    pointers: Vec<wl_pointer::WlPointer>,
    active_surface: Option<wl_surface::WlSurface>,
    serial: u32,
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

struct AttachedBuffer {
    resource: wl_buffer::WlBuffer,
    data: Arc<ShmBuffer>,
}

#[derive(Default)]
struct SurfaceState {
    pending_buffer: Mutex<Option<AttachedBuffer>>,
    frame_callbacks: Mutex<Vec<wl_callback::WlCallback>>,
}

#[derive(Debug)]
struct ClientLog;

impl ClientData for ClientLog {
    fn disconnected(&self, client_id: ClientId, reason: DisconnectReason) {
        log::warn!("Wayland client {client_id:?} disconnected: {reason:?}");
    }
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
        state: &mut Self,
        _client: &wayland_server::Client,
        _resource: &wl_surface::WlSurface,
        request: wl_surface::Request,
        data: &Arc<SurfaceState>,
        _handle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            wl_surface::Request::Attach { buffer, .. } => {
                let buffer = buffer.and_then(|resource| {
                    resource
                        .data::<Arc<ShmBuffer>>()
                        .cloned()
                        .map(|data| AttachedBuffer { resource, data })
                });
                log::trace!("surface attach: shared_memory={}", buffer.is_some());
                *data.pending_buffer.lock().expect("surface buffer lock") = buffer;
            }
            wl_surface::Request::Frame { callback } => {
                let callback = data_init.init::<wl_callback::WlCallback, _>(callback, ());
                data.frame_callbacks
                    .lock()
                    .expect("surface callbacks lock")
                    .push(callback);
            }
            wl_surface::Request::Commit => {
                state.active_surface = Some(_resource.clone());
                if let Some(buffer) = data
                    .pending_buffer
                    .lock()
                    .expect("surface buffer lock")
                    .as_ref()
                {
                    let buffer_data = &buffer.data;
                    let length = buffer_data.stride as usize * buffer_data.height as usize;
                    let mut pixels = vec![0_u8; length];
                    let mut read = 0_usize;
                    while read < length {
                        match buffer_data
                            .pool
                            .file
                            .read_at(&mut pixels[read..], buffer_data.offset + read as u64)
                        {
                            Ok(0) => break,
                            Ok(bytes) => read += bytes,
                            Err(error) => {
                                log::warn!("read shared buffer: {error}");
                                break;
                            }
                        }
                    }
                    if read == length {
                        let frame = CommittedFrame {
                            width: buffer_data.width as u32,
                            height: buffer_data.height as u32,
                            stride: buffer_data.stride as u32,
                            pixels,
                        };
                        let _ = state.frame_tx.try_send(frame);
                        log::trace!(
                            "shared buffer committed: {}x{} stride={}",
                            buffer_data.width,
                            buffer_data.height,
                            buffer_data.stride
                        );
                    }
                    buffer.resource.release();
                }
                for callback in data
                    .frame_callbacks
                    .lock()
                    .expect("surface callbacks lock")
                    .drain(..)
                {
                    callback.done(0);
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_callback::WlCallback, ()> for State {
    fn request(
        _state: &mut Self,
        _client: &wayland_server::Client,
        _resource: &wl_callback::WlCallback,
        _request: wl_callback::Request,
        _data: &(),
        _handle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
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

impl GlobalDispatch<wl_seat::WlSeat, ()> for State {
    fn bind(
        _state: &mut Self,
        _handle: &DisplayHandle,
        _client: &wayland_server::Client,
        resource: New<wl_seat::WlSeat>,
        _global_data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        let resource = data_init.init(resource, ());
        resource.capabilities(wl_seat::Capability::Pointer);
        resource.name("PvrWay touch pointer".to_string());
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for State {
    fn request(
        state: &mut Self,
        _client: &wayland_server::Client,
        _resource: &wl_seat::WlSeat,
        request: wl_seat::Request,
        _data: &(),
        _handle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        if let wl_seat::Request::GetPointer { id } = request {
            let pointer = data_init.init::<wl_pointer::WlPointer, _>(id, ());
            state.pointers.push(pointer);
        }
    }
}

impl Dispatch<wl_pointer::WlPointer, ()> for State {
    fn request(
        _state: &mut Self,
        _client: &wayland_server::Client,
        _resource: &wl_pointer::WlPointer,
        _request: wl_pointer::Request,
        _data: &(),
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
            toplevel.configure(1600, 720, Vec::new());
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

pub fn run_foreground(frame_tx: SyncSender<CommittedFrame>) -> Result<(), String> {
    let socket = ListeningSocket::bind("pvrway-proxy.sock")
        .map_err(|error| format!("bind proxy Wayland socket: {error:?}"))?;
    let input_rx = spawn_input_receiver()?;
    run(socket, frame_tx, input_rx);
    Ok(())
}

fn run(
    socket: ListeningSocket,
    frame_tx: SyncSender<CommittedFrame>,
    input_rx: Receiver<PointerPacket>,
) {
    let mut display = match Display::<State>::new() {
        Ok(display) => display,
        Err(error) => {
            log::error!("create Wayland display: {error:?}");
            return;
        }
    };
    let mut state = State {
        globals: Vec::new(),
        frame_tx,
        input_rx,
        pointers: Vec::new(),
        active_surface: None,
        serial: 1,
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
    state.globals.push(
        display
            .handle()
            .create_global::<State, wl_seat::WlSeat, ()>(7, ()),
    );
    let client_data: Arc<dyn ClientData> = Arc::new(ClientLog);

    loop {
        while let Ok(Some(stream)) = socket.accept() {
            if let Err(error) = display.handle().insert_client(stream, client_data.clone()) {
                log::warn!("accept Wayland client: {error}");
            }
        }
        let dispatch = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            display.dispatch_clients(&mut state)
        }));
        match dispatch {
            Ok(result) => match result {
                Ok(_count) => {}
                Err(error) => log::warn!("dispatch Wayland clients: {error}"),
            },
            Err(_) => {
                log::error!("Wayland dispatch panicked; server loop is stopping");
                return;
            }
        }
        if let Err(error) = display.flush_clients() {
            log::warn!("flush Wayland clients: {error}");
        }
        while let Ok(packet) = state.input_rx.try_recv() {
            state.dispatch_pointer(packet);
        }
        thread::sleep(Duration::from_millis(4));
    }
}

impl State {
    fn dispatch_pointer(&mut self, packet: PointerPacket) {
        let Some(surface) = self.active_surface.clone() else {
            return;
        };
        let x = packet.x.clamp(0.0, 1599.0) as f64;
        let y = packet.y.clamp(0.0, 719.0) as f64;
        self.serial = self.serial.wrapping_add(1);
        let serial = self.serial;
        for pointer in &self.pointers {
            match packet.action {
                PointerAction::Down => {
                    pointer.enter(serial, &surface, x, y);
                    pointer.motion(packet.time, x, y);
                    pointer.button(serial, packet.time, 0x110, wl_pointer::ButtonState::Pressed);
                    pointer.frame();
                }
                PointerAction::Motion => {
                    pointer.motion(packet.time, x, y);
                    pointer.frame();
                }
                PointerAction::Up => {
                    pointer.motion(packet.time, x, y);
                    pointer.button(
                        serial,
                        packet.time,
                        0x110,
                        wl_pointer::ButtonState::Released,
                    );
                    pointer.leave(serial, &surface);
                    pointer.frame();
                }
                PointerAction::Cancel => {
                    pointer.leave(serial, &surface);
                    pointer.frame();
                }
            }
        }
    }
}

fn spawn_input_receiver() -> Result<Receiver<PointerPacket>, String> {
    let _ = fs::remove_file(PROXY_INPUT_SOCKET);
    let listener = UnixListener::bind(PROXY_INPUT_SOCKET)
        .map_err(|error| format!("bind input socket: {error}"))?;
    fs::set_permissions(PROXY_INPUT_SOCKET, fs::Permissions::from_mode(0o666))
        .map_err(|error| format!("set input socket permissions: {error}"))?;
    let (input_tx, input_rx) = mpsc::sync_channel(64);
    thread::Builder::new()
        .name("pvrway-input-receiver".to_string())
        .spawn(move || {
            for connection in listener.incoming() {
                match connection.and_then(read_pointer) {
                    Ok(packet) => {
                        let _ = input_tx.try_send(packet);
                    }
                    Err(error) => log::warn!("receive Android pointer: {error}"),
                }
            }
        })
        .map_err(|error| format!("spawn input receiver: {error}"))?;
    Ok(input_rx)
}
