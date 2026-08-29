use std::path::PathBuf;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use wayland_server::backend::ClientData;
use wayland_server::protocol::{wl_compositor, wl_region, wl_surface};
use wayland_server::{
    DataInit, Dispatch, Display, DisplayHandle, GlobalDispatch, ListeningSocket, New,
};

#[derive(Default)]
pub struct State;

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
                data_init.init::<wl_surface::WlSurface, _>(id, ());
            }
            wl_compositor::Request::CreateRegion { id } => {
                data_init.init::<wl_region::WlRegion, _>(id, ());
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_surface::WlSurface, ()> for State {
    fn request(
        _state: &mut Self,
        _client: &wayland_server::Client,
        _resource: &wl_surface::WlSurface,
        _request: wl_surface::Request,
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
    display
        .handle()
        .create_global::<State, wl_compositor::WlCompositor, ()>(5, ());
    let mut state = State;
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
