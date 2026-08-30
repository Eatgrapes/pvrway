#[path = "../wayland_host.rs"]
mod wayland_host;

fn main() {
    if let Err(error) = wayland_host::run_foreground() {
        eprintln!("pvrway-proxy: {error}");
        std::process::exit(1);
    }
}
