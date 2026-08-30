use std::io;
#[cfg(any(not(target_os = "android"), feature = "proxy"))]
use std::io::Read;
#[cfg(target_os = "android")]
use std::io::Write;

#[cfg(target_os = "android")]
pub const ANDROID_INPUT_SOCKET: &str = "/data/user/0/io.eatgrapes.pvrway/files/pvrway-input.sock";
#[cfg(any(not(target_os = "android"), feature = "proxy"))]
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

#[derive(Clone, Copy, Debug)]
pub struct KeyPacket {
    pub pressed: bool,
    pub time: u32,
    pub key: u32,
}

#[derive(Clone, Copy, Debug)]
pub enum InputPacket {
    Pointer(PointerPacket),
    Key(KeyPacket),
}

#[cfg(target_os = "android")]
pub fn write_input(mut writer: impl Write, packet: InputPacket) -> io::Result<()> {
    let mut bytes = [0_u8; 14];
    let length = match packet {
        InputPacket::Pointer(packet) => {
            bytes[0] = 1;
            bytes[1] = packet.action as u8;
            bytes[2..6].copy_from_slice(&packet.time.to_le_bytes());
            bytes[6..10].copy_from_slice(&packet.x.to_le_bytes());
            bytes[10..14].copy_from_slice(&packet.y.to_le_bytes());
            14
        }
        InputPacket::Key(packet) => {
            bytes[0] = 2;
            bytes[1] = u8::from(packet.pressed);
            bytes[2..6].copy_from_slice(&packet.time.to_le_bytes());
            bytes[6..10].copy_from_slice(&packet.key.to_le_bytes());
            10
        }
    };
    writer.write_all(&bytes[..length])
}

#[cfg(any(not(target_os = "android"), feature = "proxy"))]
pub fn read_input(mut reader: impl Read) -> io::Result<InputPacket> {
    let mut kind = [0_u8; 1];
    reader.read_exact(&mut kind)?;
    let mut bytes = [0_u8; 13];
    let length = match kind[0] {
        1 => 13,
        2 => 9,
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid input packet",
            ));
        }
    };
    reader.read_exact(&mut bytes[..length])?;
    if kind[0] == 2 {
        return Ok(InputPacket::Key(KeyPacket {
            pressed: bytes[0] != 0,
            time: u32::from_le_bytes(bytes[1..5].try_into().unwrap()),
            key: u32::from_le_bytes(bytes[5..9].try_into().unwrap()),
        }));
    }
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
    Ok(InputPacket::Pointer(PointerPacket {
        action,
        time: u32::from_le_bytes(bytes[1..5].try_into().unwrap()),
        x: f32::from_le_bytes(bytes[5..9].try_into().unwrap()),
        y: f32::from_le_bytes(bytes[9..13].try_into().unwrap()),
    }))
}
