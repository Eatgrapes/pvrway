#![cfg(target_os = "android")]

use std::sync::OnceLock;
use std::time::Duration;

use android_activity::input::{ImeOptions, InputEvent, InputType, MotionAction, TextInputAction};
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
    app.set_ime_editor_info(
        InputType::TYPE_CLASS_TEXT | InputType::TYPE_TEXT_FLAG_NO_SUGGESTIONS,
        TextInputAction::None,
        ImeOptions::IME_FLAG_NO_FULLSCREEN,
    );

    let mut destroyed = false;
    let mut egl_host = None;
    let frame_rx = frame_receiver::spawn();
    let input_tx = input_sender::spawn();
    let mut soft_keyboard_pressed = false;
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
            while input.next(|event| match event {
                InputEvent::MotionEvent(event) => {
                    let pointer = event.pointer_at_index(0);
                    if matches!(event.action(), MotionAction::Down)
                        && pointer.raw_x() >= 1450.0
                        && pointer.raw_y() >= 630.0
                    {
                        soft_keyboard_pressed = true;
                        app.show_soft_input(true);
                        InputStatus::Handled
                    } else if soft_keyboard_pressed {
                        if matches!(event.action(), MotionAction::Up | MotionAction::Cancel) {
                            soft_keyboard_pressed = false;
                        }
                        InputStatus::Handled
                    } else {
                        let action = match event.action() {
                            MotionAction::Down => Some(input_protocol::PointerAction::Down),
                            MotionAction::Up => Some(input_protocol::PointerAction::Up),
                            MotionAction::Move => Some(input_protocol::PointerAction::Motion),
                            MotionAction::Cancel => Some(input_protocol::PointerAction::Cancel),
                            _ => None,
                        };
                        if let Some(action) = action {
                            let packet = input_protocol::PointerPacket {
                                action,
                                time: (event.event_time() / 1_000_000) as u32,
                                x: pointer.raw_x(),
                                y: pointer.raw_y(),
                            };
                            let _ = input_tx.try_send(input_protocol::InputPacket::Pointer(packet));
                            InputStatus::Handled
                        } else {
                            InputStatus::Unhandled
                        }
                    }
                }
                InputEvent::KeyEvent(event) => {
                    let key = map_android_key(event.key_code().into());
                    if let Some(key) = key {
                        let pressed =
                            matches!(event.action(), android_activity::input::KeyAction::Down);
                        let packet = input_protocol::KeyPacket {
                            pressed,
                            time: (event.event_time() / 1_000_000) as u32,
                            key,
                        };
                        let _ = input_tx.try_send(input_protocol::InputPacket::Key(packet));
                        InputStatus::Handled
                    } else {
                        InputStatus::Unhandled
                    }
                }
                _ => InputStatus::Unhandled,
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

fn map_android_key(key: u32) -> Option<u32> {
    match key {
        29..=54 => Some(key + 1),
        7..=16 => Some(if key == 7 { 11 } else { key - 6 }),
        62 => Some(57),
        66 => Some(28),
        67 => Some(14),
        61 => Some(15),
        111 => Some(1),
        59 => Some(42),
        60 => Some(54),
        113 => Some(29),
        114 => Some(97),
        57 => Some(56),
        58 => Some(100),
        19 => Some(103),
        20 => Some(108),
        21 => Some(105),
        22 => Some(106),
        23 => Some(28),
        _ => None,
    }
}
