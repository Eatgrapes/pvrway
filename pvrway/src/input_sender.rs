use std::os::unix::net::UnixStream;
use std::sync::mpsc::{self, SyncSender};
use std::thread;
use std::time::Duration;

use crate::input_protocol::{ANDROID_INPUT_SOCKET, InputPacket, write_input};

pub fn spawn() -> SyncSender<InputPacket> {
    let (input_tx, input_rx) = mpsc::sync_channel(64);
    thread::Builder::new()
        .name("pvrway-input-sender".to_string())
        .spawn(move || {
            while let Ok(packet) = input_rx.recv() {
                loop {
                    if let Ok(stream) = UnixStream::connect(ANDROID_INPUT_SOCKET) {
                        if write_input(stream, packet).is_ok() {
                            break;
                        }
                    }
                    thread::sleep(Duration::from_millis(100));
                }
            }
        })
        .expect("spawn input sender thread");
    input_tx
}
