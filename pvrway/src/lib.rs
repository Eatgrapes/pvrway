#![cfg(target_os = "android")]

use std::sync::OnceLock;
use std::time::Duration;

use android_activity::{AndroidApp, InputStatus, MainEvent, PollEvent};

mod egl_host;

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
            while input.next(|_| InputStatus::Unhandled) {}
        }
    }

    log::info!("PvrWay native activity stopped");
}
