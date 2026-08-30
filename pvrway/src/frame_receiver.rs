use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::Duration;

use crate::frame_protocol::{ANDROID_FRAME_SOCKET, CommittedFrame, read_frame};

pub fn spawn() -> Receiver<CommittedFrame> {
    let (frame_tx, frame_rx) = mpsc::sync_channel(2);
    thread::Builder::new()
        .name("pvrway-frame-receiver".to_string())
        .spawn(move || {
            loop {
                let _ = fs::remove_file(ANDROID_FRAME_SOCKET);
                match UnixListener::bind(ANDROID_FRAME_SOCKET) {
                    Ok(listener) => {
                        if let Err(error) = fs::set_permissions(
                            ANDROID_FRAME_SOCKET,
                            fs::Permissions::from_mode(0o666),
                        ) {
                            log::warn!("set frame socket permissions: {error}");
                        }
                        log::info!("frame receiver ready at {ANDROID_FRAME_SOCKET}");
                        for connection in listener.incoming() {
                            match connection.and_then(read_frame) {
                                Ok(frame) => {
                                    let _ = frame_tx.try_send(frame);
                                }
                                Err(error) => log::warn!("receive proxy frame: {error}"),
                            }
                        }
                    }
                    Err(error) => {
                        log::warn!("bind Unix frame receiver: {error}");
                        thread::sleep(Duration::from_secs(1));
                    }
                }
            }
        })
        .expect("spawn frame receiver thread");
    frame_rx
}
