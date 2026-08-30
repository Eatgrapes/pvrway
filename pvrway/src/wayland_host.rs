use std::fs::{self, File};
use std::os::unix::fs::FileExt;
use std::os::unix::fs::MetadataExt;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::io::AsFd;
use std::os::unix::net::UnixListener;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::thread;
use std::time::{Duration, Instant};

use wayland_protocols::wp::pointer_constraints::zv1 as pointer_constraints;
use wayland_protocols::wp::relative_pointer::zv1 as relative_pointer;
use wayland_protocols::wp::{linux_dmabuf, presentation_time, viewporter};
use wayland_protocols::xdg::shell::server::{xdg_surface, xdg_toplevel, xdg_wm_base};
use wayland_server::backend::{ClientData, ClientId, DisconnectReason, GlobalId};
use wayland_server::protocol::{
    wl_buffer, wl_callback, wl_compositor, wl_keyboard, wl_output, wl_pointer, wl_region, wl_seat,
    wl_shm, wl_shm_pool, wl_subcompositor, wl_subsurface, wl_surface,
};
use wayland_server::{
    DataInit, Dispatch, Display, DisplayHandle, GlobalDispatch, ListeningSocket, New, Resource,
};

use crate::frame_protocol::CommittedFrame;
use crate::input_protocol::{
    InputPacket, PROXY_INPUT_SOCKET, PointerAction, PointerPacket, read_input,
};

pub struct State {
    globals: Vec<GlobalId>,
    frame_tx: SyncSender<CommittedFrame>,
    input_rx: Receiver<InputPacket>,
    pointers: Vec<wl_pointer::WlPointer>,
    keyboards: Vec<wl_keyboard::WlKeyboard>,
    wm_bases: Vec<xdg_wm_base::XdgWmBase>,
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

#[derive(Default)]
struct DmabufParams {
    planes: Mutex<Vec<DmabufPlane>>,
}

struct DmabufPlane {
    file: File,
    plane_index: u32,
    offset: u32,
    stride: u32,
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

impl GlobalDispatch<wl_subcompositor::WlSubcompositor, ()> for State {
    fn bind(
        _state: &mut Self,
        _handle: &DisplayHandle,
        _client: &wayland_server::Client,
        resource: New<wl_subcompositor::WlSubcompositor>,
        _global_data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }
}

impl Dispatch<wl_subcompositor::WlSubcompositor, ()> for State {
    fn request(
        _state: &mut Self,
        _client: &wayland_server::Client,
        _resource: &wl_subcompositor::WlSubcompositor,
        request: wl_subcompositor::Request,
        _data: &(),
        _handle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        if let wl_subcompositor::Request::GetSubsurface { id, .. } = request {
            data_init.init::<wl_subsurface::WlSubsurface, _>(id, ());
        }
    }
}

impl Dispatch<wl_subsurface::WlSubsurface, ()> for State {
    fn request(
        _state: &mut Self,
        _client: &wayland_server::Client,
        _resource: &wl_subsurface::WlSubsurface,
        _request: wl_subsurface::Request,
        _data: &(),
        _handle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
    }
}

impl GlobalDispatch<viewporter::server::wp_viewporter::WpViewporter, ()> for State {
    fn bind(
        _state: &mut Self,
        _handle: &DisplayHandle,
        _client: &wayland_server::Client,
        resource: New<viewporter::server::wp_viewporter::WpViewporter>,
        _global_data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }
}

impl Dispatch<viewporter::server::wp_viewporter::WpViewporter, ()> for State {
    fn request(
        _state: &mut Self,
        _client: &wayland_server::Client,
        _resource: &viewporter::server::wp_viewporter::WpViewporter,
        request: viewporter::server::wp_viewporter::Request,
        _data: &(),
        _handle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        if let viewporter::server::wp_viewporter::Request::GetViewport { id, .. } = request {
            data_init.init::<viewporter::server::wp_viewport::WpViewport, _>(id, ());
        }
    }
}

impl Dispatch<viewporter::server::wp_viewport::WpViewport, ()> for State {
    fn request(
        _state: &mut Self,
        _client: &wayland_server::Client,
        _resource: &viewporter::server::wp_viewport::WpViewport,
        _request: viewporter::server::wp_viewport::Request,
        _data: &(),
        _handle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
    }
}

impl GlobalDispatch<presentation_time::server::wp_presentation::WpPresentation, ()> for State {
    fn bind(
        _state: &mut Self,
        _handle: &DisplayHandle,
        _client: &wayland_server::Client,
        resource: New<presentation_time::server::wp_presentation::WpPresentation>,
        _global_data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        let resource = data_init.init(resource, ());
        resource.clock_id(1);
    }
}

impl Dispatch<presentation_time::server::wp_presentation::WpPresentation, ()> for State {
    fn request(
        _state: &mut Self,
        _client: &wayland_server::Client,
        _resource: &presentation_time::server::wp_presentation::WpPresentation,
        request: presentation_time::server::wp_presentation::Request,
        _data: &(),
        _handle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        if let presentation_time::server::wp_presentation::Request::Feedback { callback, .. } =
            request
        {
            data_init.init::<presentation_time::server::wp_presentation_feedback::WpPresentationFeedback, _>(callback, ());
        }
    }
}

impl Dispatch<presentation_time::server::wp_presentation_feedback::WpPresentationFeedback, ()>
    for State
{
    fn request(
        _state: &mut Self,
        _client: &wayland_server::Client,
        _resource: &presentation_time::server::wp_presentation_feedback::WpPresentationFeedback,
        _request: presentation_time::server::wp_presentation_feedback::Request,
        _data: &(),
        _handle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
    }
}

impl
    GlobalDispatch<
        pointer_constraints::server::zwp_pointer_constraints_v1::ZwpPointerConstraintsV1,
        (),
    > for State
{
    fn bind(
        _state: &mut Self,
        _handle: &DisplayHandle,
        _client: &wayland_server::Client,
        resource: New<
            pointer_constraints::server::zwp_pointer_constraints_v1::ZwpPointerConstraintsV1,
        >,
        _global_data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }
}

impl Dispatch<pointer_constraints::server::zwp_pointer_constraints_v1::ZwpPointerConstraintsV1, ()>
    for State
{
    fn request(
        _state: &mut Self,
        _client: &wayland_server::Client,
        _resource: &pointer_constraints::server::zwp_pointer_constraints_v1::ZwpPointerConstraintsV1,
        request: pointer_constraints::server::zwp_pointer_constraints_v1::Request,
        _data: &(),
        _handle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            pointer_constraints::server::zwp_pointer_constraints_v1::Request::LockPointer {
                id,
                ..
            } => {
                data_init.init::<pointer_constraints::server::zwp_locked_pointer_v1::ZwpLockedPointerV1, _>(id, ());
            }
            pointer_constraints::server::zwp_pointer_constraints_v1::Request::ConfinePointer {
                id,
                ..
            } => {
                data_init.init::<pointer_constraints::server::zwp_confined_pointer_v1::ZwpConfinedPointerV1, _>(id, ());
            }
            _ => {}
        }
    }
}

impl Dispatch<pointer_constraints::server::zwp_locked_pointer_v1::ZwpLockedPointerV1, ()>
    for State
{
    fn request(
        _state: &mut Self,
        _client: &wayland_server::Client,
        _resource: &pointer_constraints::server::zwp_locked_pointer_v1::ZwpLockedPointerV1,
        _request: pointer_constraints::server::zwp_locked_pointer_v1::Request,
        _data: &(),
        _handle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
    }
}

impl Dispatch<pointer_constraints::server::zwp_confined_pointer_v1::ZwpConfinedPointerV1, ()>
    for State
{
    fn request(
        _state: &mut Self,
        _client: &wayland_server::Client,
        _resource: &pointer_constraints::server::zwp_confined_pointer_v1::ZwpConfinedPointerV1,
        _request: pointer_constraints::server::zwp_confined_pointer_v1::Request,
        _data: &(),
        _handle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
    }
}

impl
    GlobalDispatch<
        relative_pointer::server::zwp_relative_pointer_manager_v1::ZwpRelativePointerManagerV1,
        (),
    > for State
{
    fn bind(
        _state: &mut Self,
        _handle: &DisplayHandle,
        _client: &wayland_server::Client,
        resource: New<
            relative_pointer::server::zwp_relative_pointer_manager_v1::ZwpRelativePointerManagerV1,
        >,
        _global_data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }
}

impl
    Dispatch<
        relative_pointer::server::zwp_relative_pointer_manager_v1::ZwpRelativePointerManagerV1,
        (),
    > for State
{
    fn request(
        _state: &mut Self,
        _client: &wayland_server::Client,
        _resource: &relative_pointer::server::zwp_relative_pointer_manager_v1::ZwpRelativePointerManagerV1,
        request: relative_pointer::server::zwp_relative_pointer_manager_v1::Request,
        _data: &(),
        _handle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        if let relative_pointer::server::zwp_relative_pointer_manager_v1::Request::GetRelativePointer { id, .. } = request {
            data_init.init::<relative_pointer::server::zwp_relative_pointer_v1::ZwpRelativePointerV1, _>(id, ());
        }
    }
}

impl Dispatch<relative_pointer::server::zwp_relative_pointer_v1::ZwpRelativePointerV1, ()>
    for State
{
    fn request(
        _state: &mut Self,
        _client: &wayland_server::Client,
        _resource: &relative_pointer::server::zwp_relative_pointer_v1::ZwpRelativePointerV1,
        _request: relative_pointer::server::zwp_relative_pointer_v1::Request,
        _data: &(),
        _handle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
    }
}

impl GlobalDispatch<linux_dmabuf::zv1::server::zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1, ()>
    for State
{
    fn bind(
        _state: &mut Self,
        _handle: &DisplayHandle,
        _client: &wayland_server::Client,
        resource: New<linux_dmabuf::zv1::server::zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1>,
        _global_data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        let resource = data_init.init(resource, ());
        resource.format(0x34325241);
    }
}

impl Dispatch<linux_dmabuf::zv1::server::zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1, ()> for State {
    fn request(
        _state: &mut Self,
        _client: &wayland_server::Client,
        _resource: &linux_dmabuf::zv1::server::zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1,
        request: linux_dmabuf::zv1::server::zwp_linux_dmabuf_v1::Request,
        _data: &(),
        _handle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            linux_dmabuf::zv1::server::zwp_linux_dmabuf_v1::Request::CreateParams { params_id } => {
                data_init.init::<linux_dmabuf::zv1::server::zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1, _>(params_id, Arc::new(DmabufParams::default()));
            }
            linux_dmabuf::zv1::server::zwp_linux_dmabuf_v1::Request::GetDefaultFeedback { id }
            | linux_dmabuf::zv1::server::zwp_linux_dmabuf_v1::Request::GetSurfaceFeedback {
                id,
                ..
            } => {
                let feedback = data_init.init::<linux_dmabuf::zv1::server::zwp_linux_dmabuf_feedback_v1::ZwpLinuxDmabufFeedbackV1, _>(id, ());
                if let Err(error) = send_dmabuf_feedback(&feedback) {
                    log::warn!("send dma-buf feedback: {error}");
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<linux_dmabuf::zv1::server::zwp_linux_dmabuf_feedback_v1::ZwpLinuxDmabufFeedbackV1, ()>
    for State
{
    fn request(
        _state: &mut Self,
        _client: &wayland_server::Client,
        _resource: &linux_dmabuf::zv1::server::zwp_linux_dmabuf_feedback_v1::ZwpLinuxDmabufFeedbackV1,
        _request: linux_dmabuf::zv1::server::zwp_linux_dmabuf_feedback_v1::Request,
        _data: &(),
        _handle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
    }
}

fn send_dmabuf_feedback(
    feedback: &linux_dmabuf::zv1::server::zwp_linux_dmabuf_feedback_v1::ZwpLinuxDmabufFeedbackV1,
) -> std::io::Result<()> {
    let device = std::fs::metadata("/dev/dri/card0")?.rdev();
    let device_bytes = device.to_ne_bytes().to_vec();
    let path = "/tmp/pvrway-dmabuf-formats";
    let mut formats = Vec::with_capacity(32);
    for format in [0x34325241_u32, 0x34325258_u32] {
        formats.extend_from_slice(&format.to_ne_bytes());
        formats.extend_from_slice(&0_u32.to_ne_bytes());
        formats.extend_from_slice(&0_u64.to_ne_bytes());
    }
    static FORMAT_FILE: OnceLock<File> = OnceLock::new();
    if FORMAT_FILE.get().is_none() {
        std::fs::write(path, &formats)?;
        let _ = FORMAT_FILE.set(File::open(path)?);
    }
    let file = FORMAT_FILE
        .get()
        .ok_or_else(|| std::io::Error::other("dma-buf format table unavailable"))?;
    feedback.main_device(device_bytes.clone());
    feedback.format_table(file.as_fd(), formats.len() as u32);
    feedback.tranche_target_device(device_bytes);
    feedback.tranche_formats(vec![0, 0, 1, 0]);
    feedback.tranche_flags(
        linux_dmabuf::zv1::server::zwp_linux_dmabuf_feedback_v1::TrancheFlags::empty(),
    );
    feedback.tranche_done();
    feedback.done();
    Ok(())
}

impl
    Dispatch<
        linux_dmabuf::zv1::server::zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1,
        Arc<DmabufParams>,
    > for State
{
    fn request(
        _state: &mut Self,
        _client: &wayland_server::Client,
        _resource: &linux_dmabuf::zv1::server::zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1,
        request: linux_dmabuf::zv1::server::zwp_linux_buffer_params_v1::Request,
        data: &Arc<DmabufParams>,
        _handle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            linux_dmabuf::zv1::server::zwp_linux_buffer_params_v1::Request::Add {
                fd,
                plane_idx,
                offset,
                stride,
                ..
            } => {
                data.planes
                    .lock()
                    .expect("dma-buf planes lock")
                    .push(DmabufPlane {
                        file: File::from(fd),
                        plane_index: plane_idx,
                        offset,
                        stride,
                    });
            }
            linux_dmabuf::zv1::server::zwp_linux_buffer_params_v1::Request::CreateImmed {
                buffer_id,
                width,
                height,
                ..
            } => {
                let planes = data.planes.lock().expect("dma-buf planes lock");
                if let Some(plane) = planes.iter().find(|plane| plane.plane_index == 0) {
                    if let Ok(file) = plane.file.try_clone() {
                        data_init.init::<wl_buffer::WlBuffer, _>(
                            buffer_id,
                            Arc::new(ShmBuffer {
                                pool: Arc::new(ShmPool { file }),
                                offset: plane.offset as u64,
                                width,
                                height,
                                stride: plane.stride as i32,
                            }),
                        );
                    }
                }
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
                state.serial = state.serial.wrapping_add(1);
                for keyboard in &state.keyboards {
                    if keyboard.id().same_client_as(&_resource.id()) {
                        keyboard.enter(state.serial, _resource, Vec::new());
                    }
                }
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
        resource.capabilities(wl_seat::Capability::Pointer | wl_seat::Capability::Keyboard);
        resource.name("PvrWay touch and keyboard".to_string());
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
        match request {
            wl_seat::Request::GetPointer { id } => {
                let pointer = data_init.init::<wl_pointer::WlPointer, _>(id, ());
                state.pointers.push(pointer);
            }
            wl_seat::Request::GetKeyboard { id } => {
                let keyboard = data_init.init::<wl_keyboard::WlKeyboard, _>(id, ());
                if let Err(error) = send_keymap(&keyboard) {
                    log::warn!("send keyboard keymap: {error}");
                }
                if let Some(surface) = state.active_surface.clone() {
                    if keyboard.id().same_client_as(&surface.id()) {
                        keyboard.enter(state.serial, &surface, Vec::new());
                    }
                }
                state.keyboards.push(keyboard);
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_keyboard::WlKeyboard, ()> for State {
    fn request(
        _state: &mut Self,
        _client: &wayland_server::Client,
        _resource: &wl_keyboard::WlKeyboard,
        _request: wl_keyboard::Request,
        _data: &(),
        _handle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
    }
}

fn send_keymap(keyboard: &wl_keyboard::WlKeyboard) -> std::io::Result<()> {
    let path = "/tmp/pvrway-keymap.xkb";
    let keymap = r#"xkb_keymap {
 xkb_keycodes "evdev" { minimum = 8; maximum = 255; };
 xkb_types "complete" { virtual_modifiers NumLock; type "PC_SUPER_LEVEL2" { modifiers = Shift+NumLock; map[None] = Level1; map[Shift] = Level2; map[NumLock] = Level2; map[Shift+NumLock] = Level1; level_name[Level1] = "Base"; level_name[Level2] = "Shift"; }; };
 xkb_compatibility "complete" { interpret Shift_L+AnyOfOrNone(all) { action = SetMods(modifiers=Shift); }; interpret Shift_R+AnyOfOrNone(all) { action = SetMods(modifiers=Shift); }; };
 xkb_symbols "pc" {
  key <ESC> { [ Escape ] }; key <AE01> { [ 1, exclam ] }; key <AE02> { [ 2, at ] }; key <AE03> { [ 3, numbersign ] }; key <AE04> { [ 4, dollar ] }; key <AE05> { [ 5, percent ] }; key <AE06> { [ 6, asciicircum ] }; key <AE07> { [ 7, ampersand ] }; key <AE08> { [ 8, asterisk ] }; key <AE09> { [ 9, parenleft ] }; key <AE10> { [ 0, parenright ] };
  key <AD01> { [ q, Q ] }; key <AD02> { [ w, W ] }; key <AD03> { [ e, E ] }; key <AD04> { [ r, R ] }; key <AD05> { [ t, T ] }; key <AD06> { [ y, Y ] }; key <AD07> { [ u, U ] }; key <AD08> { [ i, I ] }; key <AD09> { [ o, O ] }; key <AD10> { [ p, P ] };
  key <AC01> { [ a, A ] }; key <AC02> { [ s, S ] }; key <AC03> { [ d, D ] }; key <AC04> { [ f, F ] }; key <AC05> { [ g, G ] }; key <AC06> { [ h, H ] }; key <AC07> { [ j, J ] }; key <AC08> { [ k, K ] }; key <AC09> { [ l, L ] };
  key <AB01> { [ z, Z ] }; key <AB02> { [ x, X ] }; key <AB03> { [ c, C ] }; key <AB04> { [ v, V ] }; key <AB05> { [ b, B ] }; key <AB06> { [ n, N ] }; key <AB07> { [ m, M ] };
  key <AC11> { [ apostrophe, quotedbl ] }; key <AB08> { [ comma, less ] }; key <AB09> { [ period, greater ] }; key <AB10> { [ slash, question ] }; key <BKSL> { [ backslash, bar ] }; key <SPCE> { [ space ] }; key <TAB> { [ Tab ] }; key <RTRN> { [ Return ] }; key <BKSP> { [ BackSpace ] };
  key <LFSH> { [ Shift_L ] }; key <RTSH> { [ Shift_R ] }; key <LCTL> { [ Control_L ] }; key <RCTL> { [ Control_R ] }; key <LALT> { [ Alt_L ] }; key <RALT> { [ Alt_R ] }; key <UP> { [ Up ] }; key <DOWN> { [ Down ] }; key <LEFT> { [ Left ] }; key <RGHT> { [ Right ] };
 };
};
"#;
    static KEYMAP_FILE: OnceLock<File> = OnceLock::new();
    if KEYMAP_FILE.get().is_none() {
        std::fs::write(path, keymap.as_bytes())?;
        let _ = KEYMAP_FILE.set(File::open(path)?);
    }
    let file = KEYMAP_FILE
        .get()
        .ok_or_else(|| std::io::Error::other("keyboard map unavailable"))?;
    let size = file.metadata()?.len() as u32;
    keyboard.keymap(wl_keyboard::KeymapFormat::XkbV1, file.as_fd(), size);
    Ok(())
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
        state: &mut Self,
        _handle: &DisplayHandle,
        _client: &wayland_server::Client,
        resource: New<xdg_wm_base::XdgWmBase>,
        _global_data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        let resource = data_init.init(resource, ());
        resource.ping(1);
        state.wm_bases.push(resource);
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
    input_rx: Receiver<InputPacket>,
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
        keyboards: Vec::new(),
        wm_bases: Vec::new(),
        active_surface: None,
        serial: 1,
    };
    state.globals.push(
        display
            .handle()
            .create_global::<State, wl_compositor::WlCompositor, ()>(6, ()),
    );
    state.globals.push(
        display
            .handle()
            .create_global::<State, wl_subcompositor::WlSubcompositor, ()>(1, ()),
    );
    state.globals.push(
        display
            .handle()
            .create_global::<State, viewporter::server::wp_viewporter::WpViewporter, ()>(1, ()),
    );
    state.globals.push(
        display
            .handle()
            .create_global::<State, presentation_time::server::wp_presentation::WpPresentation, ()>(
                1,
                (),
            ),
    );
    state.globals.push(
        display
            .handle()
            .create_global::<State, pointer_constraints::server::zwp_pointer_constraints_v1::ZwpPointerConstraintsV1, ()>(1, ()),
    );
    state.globals.push(
        display
            .handle()
            .create_global::<State, relative_pointer::server::zwp_relative_pointer_manager_v1::ZwpRelativePointerManagerV1, ()>(1, ()),
    );
    state.globals.push(
        display
            .handle()
            .create_global::<State, linux_dmabuf::zv1::server::zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1, ()>(4, ()),
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
            .create_global::<State, wl_output::WlOutput, ()>(4, ()),
    );
    state.globals.push(
        display
            .handle()
            .create_global::<State, wl_seat::WlSeat, ()>(9, ()),
    );
    let client_data: Arc<dyn ClientData> = Arc::new(ClientLog);

    let mut last_client_wakeup = Instant::now();
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
            state.dispatch_input(packet);
        }
        if state.active_surface.is_none()
            && last_client_wakeup.elapsed() >= Duration::from_millis(250)
        {
            state.serial = state.serial.wrapping_add(1);
            state.wm_bases.retain(Resource::is_alive);
            for wm_base in &state.wm_bases {
                wm_base.ping(state.serial);
            }
            last_client_wakeup = Instant::now();
        }
        thread::sleep(Duration::from_millis(4));
    }
}

impl State {
    fn dispatch_input(&mut self, packet: InputPacket) {
        if let InputPacket::Pointer(packet) = packet {
            self.dispatch_pointer(packet);
        } else if let InputPacket::Key(packet) = packet {
            self.dispatch_key(packet);
        }
    }

    fn dispatch_pointer(&mut self, packet: PointerPacket) {
        let Some(surface) = self.active_surface.clone() else {
            return;
        };
        let x = packet.x.clamp(0.0, 1599.0) as f64;
        let y = packet.y.clamp(0.0, 719.0) as f64;
        self.serial = self.serial.wrapping_add(1);
        let serial = self.serial;
        for pointer in &self.pointers {
            if !pointer.id().same_client_as(&surface.id()) {
                continue;
            }
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

    fn dispatch_key(&mut self, packet: crate::input_protocol::KeyPacket) {
        self.serial = self.serial.wrapping_add(1);
        let state = if packet.pressed {
            wl_keyboard::KeyState::Pressed
        } else {
            wl_keyboard::KeyState::Released
        };
        for keyboard in &self.keyboards {
            if self
                .active_surface
                .as_ref()
                .is_some_and(|surface| keyboard.id().same_client_as(&surface.id()))
            {
                keyboard.key(self.serial, packet.time, packet.key, state);
            }
        }
    }
}

fn spawn_input_receiver() -> Result<Receiver<InputPacket>, String> {
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
                match connection.and_then(read_input) {
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
