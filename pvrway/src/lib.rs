#![cfg(target_os = "android")]

use std::sync::{OnceLock, mpsc};
use std::time::Duration;

use android_activity::{AndroidApp, InputStatus, MainEvent, PollEvent};

mod egl_host;
mod wayland_host;

#[unsafe(no_mangle)]
fn android_main(app: AndroidApp) {
    static LOGGER: OnceLock<()> = OnceLock::new();
    LOGGER.get_or_init(|| {
        android_logger::init_once(
            android_logger::Config::default().with_max_level(log::LevelFilter::Info),
        );
    });

    log::info!("PvrWay native activity started");

    let mut destroyed = false;
    let mut egl_host = None;
    let mut wayland_started = false;
    let (frame_tx, frame_rx) = mpsc::sync_channel(1);
    while !destroyed {
        app.poll_events(Some(Duration::from_millis(100)), |event| match event {
            PollEvent::Main(MainEvent::InitWindow { .. }) => {
                if !wayland_started {
                    match wayland_host::spawn(frame_tx.clone()) {
                        Ok(()) => {
                            wayland_started = true;
                            log::info!("Wayland socket ready in the PvrWay app directory");
                        }
                        Err(error) => log::error!("Wayland server initialization failed: {error}"),
                    }
                }
                if let Some(window) = app.native_window() {
                    match egl_host::EglHost::new(&window) {
                        Ok(host) => {
                            egl_host = Some(host);
                            log::info!("PowerVR EGL host is ready");
                        }
                        Err(error) => log::error!("EGL host initialization failed: {error}"),
                    }
                }
            }
            PollEvent::Main(MainEvent::TerminateWindow { .. }) => {
                egl_host = None;
                log::info!("Android native window was released");
            }
            PollEvent::Main(MainEvent::Destroy) => {
                destroyed = true;
            }
            _ => {}
        });

        if let Ok(mut input) = app.input_events_iter() {
            while input.next(|_| InputStatus::Unhandled) {}
        }

        if frame_rx.try_recv().is_ok() {
            if let Some(host) = &egl_host {
                match host.present_wayland_frame() {
                    Ok(()) => log::info!("presented a Wayland frame through PowerVR EGL"),
                    Err(error) => log::warn!("present Wayland frame: {error}"),
                }
            }
        }
    }

    log::info!("PvrWay native activity stopped");
}
