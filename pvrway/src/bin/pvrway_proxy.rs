#[path = "../frame_protocol.rs"]
mod frame_protocol;
#[path = "../input_protocol.rs"]
mod input_protocol;
#[path = "../wayland_host.rs"]
mod wayland_host;

fn main() {
    let (frame_tx, frame_rx) = std::sync::mpsc::sync_channel(2);
    std::thread::spawn(move || send_frames(frame_rx));
    if let Err(error) = wayland_host::run_foreground(frame_tx) {
        eprintln!("pvrway-proxy: {error}");
        std::process::exit(1);
    }
}

fn send_frames(frame_rx: std::sync::mpsc::Receiver<frame_protocol::CommittedFrame>) {
    for frame in frame_rx {
        loop {
            match std::os::unix::net::UnixStream::connect(frame_protocol::PROXY_FRAME_SOCKET) {
                Ok(stream) => {
                    if frame_protocol::write_frame(stream, &frame).is_ok() {
                        break;
                    }
                }
                Err(_) => std::thread::sleep(std::time::Duration::from_millis(250)),
            }
        }
    }
}
