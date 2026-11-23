//! The serial ports this computer has, for the rig's CAT and PTT pickers
//! (M5 §6). Listed from the system's own records, never by opening a port:
//! Windows' `HARDWARE\DEVICEMAP\SERIALCOMM`, Linux's `/dev` and `/sys`.

use std::path::Path;

/// The ports, in natural order (COM3 before COM10), once each.
pub fn ports() -> Vec<String> {
    #[cfg(windows)]
    let found = windows_ports();
    #[cfg(not(windows))]
    let found = list_in(Path::new("/dev"), Path::new("/sys/class/tty"));
    sorted(found)
}

fn sorted(mut v: Vec<String>) -> Vec<String> {
    v.sort_by_key(|s| key(s));
    v.dedup();
    v
}

/// Text, then the trailing number as a number.
fn key(s: &str) -> (String, u64, String) {
    let digits = s.len() - s.trim_end_matches(|c: char| c.is_ascii_digit()).len();
    let (head, tail) = s.split_at(s.len() - digits);
    (head.to_string(), tail.parse().unwrap_or(0), s.to_string())
}

/// Linux: USB serial adapters (`ttyUSB*`, `ttyACM*`), their stable
/// `/dev/serial/by-id` names, and the built-in `ttyS*` that have a device
/// behind them (`sys_tty/ttySN/device`): most of the 32 listed have none.
pub fn list_in(dev: &Path, sys_tty: &Path) -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dev) {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let usb = name.starts_with("ttyUSB") || name.starts_with("ttyACM");
            let builtin = name.starts_with("ttyS") && sys_tty.join(&name).join("device").exists();
            if usb || builtin {
                out.push(dev.join(&name).to_string_lossy().into_owned());
            }
        }
    }
    if let Ok(entries) = std::fs::read_dir(dev.join("serial").join("by-id")) {
        for e in entries.flatten() {
            out.push(e.path().to_string_lossy().into_owned());
        }
    }
    out
}

#[cfg(windows)]
fn windows_ports() -> Vec<String> {
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegEnumValueW, RegOpenKeyExW, HKEY, HKEY_LOCAL_MACHINE, KEY_READ, REG_SZ,
    };
    let path: Vec<u16> = "HARDWARE\\DEVICEMAP\\SERIALCOMM\0".encode_utf16().collect();
    let mut hkey: HKEY = std::ptr::null_mut();
    // SAFETY: `path` is NUL-terminated and `hkey` is a valid out pointer.
    if unsafe { RegOpenKeyExW(HKEY_LOCAL_MACHINE, path.as_ptr(), 0, KEY_READ, &mut hkey) } != ERROR_SUCCESS {
        return Vec::new(); // no serial ports: the key exists only when there are some
    }
    let mut out = Vec::new();
    for i in 0..256u32 {
        let mut name = [0u16; 256];
        let mut name_len = name.len() as u32;
        let mut data = [0u16; 128];
        let mut data_len = std::mem::size_of_val(&data) as u32;
        let mut kind = 0u32;
        // SAFETY: every buffer is as long as the length passed beside it.
        let r = unsafe {
            RegEnumValueW(
                hkey,
                i,
                name.as_mut_ptr(),
                &mut name_len,
                std::ptr::null(),
                &mut kind,
                data.as_mut_ptr().cast(),
                &mut data_len,
            )
        };
        if r != ERROR_SUCCESS {
            break;
        }
        if kind == REG_SZ {
            let n = (data_len as usize / 2).min(data.len());
            let port = String::from_utf16_lossy(&data[..n]);
            let port = port.trim_end_matches('\0').trim();
            if !port.is_empty() {
                out.push(port.to_string());
            }
        }
    }
    // SAFETY: `hkey` was opened above.
    unsafe { RegCloseKey(hkey) };
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn natural_order_once_each() {
        let v = sorted(vec!["COM10".into(), "COM3".into(), "COM1".into(), "COM3".into()]);
        assert_eq!(v, ["COM1", "COM3", "COM10"]);
    }

    #[test]
    fn linux_lists_usb_by_id_and_real_ttys_only() {
        let root = tempfile::tempdir().unwrap();
        let dev = root.path().join("dev");
        let sys = root.path().join("sys");
        std::fs::create_dir_all(dev.join("serial").join("by-id")).unwrap();
        for f in ["ttyUSB0", "ttyACM1", "ttyS0", "ttyS1", "tty0", "null"] {
            std::fs::write(dev.join(f), "").unwrap();
        }
        std::fs::write(dev.join("serial").join("by-id").join("usb-FTDI_FT232R-if00-port0"), "").unwrap();
        std::fs::create_dir_all(sys.join("ttyS1").join("device")).unwrap();
        std::fs::create_dir_all(sys.join("ttyS0")).unwrap();
        let names: Vec<String> = sorted(list_in(&dev, &sys))
            .iter()
            .map(|p| Path::new(p).file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        // The stable by-id names first (/dev/serial/... sorts before /dev/tty...): they survive a re-plug.
        assert_eq!(names, ["usb-FTDI_FT232R-if00-port0", "ttyACM1", "ttyS1", "ttyUSB0"]);
    }
}
