use std::io;
#[cfg(not(target_os = "android"))]
use std::io::Read;
#[cfg(target_os = "android")]
use std::io::Write;

#[cfg(target_os = "android")]
pub const ANDROID_INPUT_SOCKET: &str = "/data/user/0/io.eatgrapes.pvrway/files/pvrway-input.sock";
#[cfg(not(target_os = "android"))]
pub const PROXY_INPUT_SOCKET: &str = "/run/pvrway-app/pvrway-input.sock";

#[derive(Clone, Copy, Debug)]
#[repr(u8)]
pub enum PointerAction {
    Down = 1,
    Up = 2,
    Motion = 3,
    Cancel = 4,
}

#[derive(Clone, Copy, Debug)]
pub struct PointerPacket {
    pub action: PointerAction,
    pub time: u32,
    pub x: f32,
    pub y: f32,
}

#[cfg(target_os = "android")]
pub fn write_pointer(mut writer: impl Write, packet: PointerPacket) -> io::Result<()> {
    let mut bytes = [0_u8; 13];
    bytes[0] = packet.action as u8;
    bytes[1..5].copy_from_slice(&packet.time.to_le_bytes());
    bytes[5..9].copy_from_slice(&packet.x.to_le_bytes());
    bytes[9..13].copy_from_slice(&packet.y.to_le_bytes());
    writer.write_all(&bytes)
}

#[cfg(not(target_os = "android"))]
pub fn read_pointer(mut reader: impl Read) -> io::Result<PointerPacket> {
    let mut bytes = [0_u8; 13];
    reader.read_exact(&mut bytes)?;
    let action = match bytes[0] {
        1 => PointerAction::Down,
        2 => PointerAction::Up,
        3 => PointerAction::Motion,
        4 => PointerAction::Cancel,
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid pointer action",
            ));
        }
    };
    Ok(PointerPacket {
        action,
        time: u32::from_le_bytes(bytes[1..5].try_into().unwrap()),
        x: f32::from_le_bytes(bytes[5..9].try_into().unwrap()),
        y: f32::from_le_bytes(bytes[9..13].try_into().unwrap()),
    })
}
