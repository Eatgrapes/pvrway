use std::io;
#[cfg(target_os = "android")]
use std::io::Read;
#[cfg(any(not(target_os = "android"), feature = "proxy"))]
use std::io::Write;

#[cfg(target_os = "android")]
pub const ANDROID_FRAME_SOCKET: &str = "/data/user/0/io.eatgrapes.pvrway/files/pvrway-frame.sock";
#[cfg(any(not(target_os = "android"), feature = "proxy"))]
pub const PROXY_FRAME_SOCKET: &str = "/run/pvrway-app/pvrway-frame.sock";
const MAGIC: [u8; 4] = *b"PVRW";
#[cfg(target_os = "android")]
const MAX_FRAME_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug)]
pub struct CommittedFrame {
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub pixels: Vec<u8>,
}

#[cfg(any(not(target_os = "android"), feature = "proxy"))]
pub fn write_frame(mut writer: impl Write, frame: &CommittedFrame) -> io::Result<()> {
    writer.write_all(&MAGIC)?;
    writer.write_all(&frame.width.to_le_bytes())?;
    writer.write_all(&frame.height.to_le_bytes())?;
    writer.write_all(&frame.stride.to_le_bytes())?;
    writer.write_all(&(frame.pixels.len() as u32).to_le_bytes())?;
    writer.write_all(&frame.pixels)?;
    writer.flush()
}

#[cfg(target_os = "android")]
pub fn read_frame(mut reader: impl Read) -> io::Result<CommittedFrame> {
    let mut header = [0_u8; 20];
    reader.read_exact(&mut header)?;
    if header[..4] != MAGIC {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid frame magic",
        ));
    }
    let width = u32::from_le_bytes(header[4..8].try_into().unwrap());
    let height = u32::from_le_bytes(header[8..12].try_into().unwrap());
    let stride = u32::from_le_bytes(header[12..16].try_into().unwrap());
    let length = u32::from_le_bytes(header[16..20].try_into().unwrap()) as usize;
    if length > MAX_FRAME_BYTES || length != stride as usize * height as usize {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid frame length",
        ));
    }
    let mut pixels = vec![0_u8; length];
    reader.read_exact(&mut pixels)?;
    Ok(CommittedFrame {
        width,
        height,
        stride,
        pixels,
    })
}
