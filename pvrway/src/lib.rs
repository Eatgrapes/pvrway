#![cfg(target_os = "android")]

use std::sync::OnceLock;
use std::time::Duration;

use android_activity::input::{InputEvent, MotionAction};
use android_activity::{AndroidApp, InputStatus, MainEvent, PollEvent};

mod egl_host;
mod frame_protocol;
mod frame_receiver;
mod input_protocol;
mod input_sender;

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
    let frame_rx = frame_receiver::spawn();
    let input_tx = input_sender::spawn();
    while !destroyed {
        app.poll_events(Some(Duration::from_millis(100)), |event| match event {
            PollEvent::Main(MainEvent::InitWindow { .. }) => {
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
            while input.next(|event| {
                if let InputEvent::MotionEvent(event) = event {
                    let action = match event.action() {
                        MotionAction::Down => Some(input_protocol::PointerAction::Down),
                        MotionAction::Up => Some(input_protocol::PointerAction::Up),
                        MotionAction::Move => Some(input_protocol::PointerAction::Motion),
                        MotionAction::Cancel => Some(input_protocol::PointerAction::Cancel),
                        _ => None,
                    };
                    if let Some(action) = action {
                        let pointer = event.pointer_at_index(0);
                        let packet = input_protocol::PointerPacket {
                            action,
                            time: (event.event_time() / 1_000_000) as u32,
                            x: pointer.raw_x(),
                            y: pointer.raw_y(),
                        };
                        let _ = input_tx.try_send(packet);
                        return InputStatus::Handled;
                    }
                }
                InputStatus::Unhandled
            }) {}
        }

        if let Ok(frame) = frame_rx.try_recv() {
            if let Some(host) = &egl_host {
                match host.present_wayland_frame(&frame) {
                    Ok(()) => log::info!("presented a Wayland frame through PowerVR EGL"),
                    Err(error) => log::warn!("present Wayland frame: {error}"),
                }
            }
        }
    }

    log::info!("PvrWay native activity stopped");
}
