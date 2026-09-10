use std::error::Error;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};

/// An exclusive claim on being *the* daemon, held for as long as the process
/// lives. The kernel drops it however the process dies, so a crash leaves no
/// stale lock behind.
#[derive(Debug)]
pub struct Lock {
    _file: File,
}

impl Lock {
    pub fn acquire() -> Result<Self, Box<dyn Error>> {
        Self::acquire_at(&path())
    }

    pub fn acquire_at(path: &Path) -> Result<Self, Box<dyn Error>> {
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
            .map_err(|err| format!("cannot open {}: {err}", path.display()))?;

        // The lock lives on the open file, not on the name, so the file is
        // never unlinked: doing so would hand the same path to a second daemon.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            let err = std::io::Error::last_os_error();
            if err.kind() != std::io::ErrorKind::WouldBlock {
                return Err(format!("cannot lock {}: {err}", path.display()).into());
            }
            return Err(match holder(&mut file) {
                Some(pid) => format!("another caffeine-daemon is already running (pid {pid})"),
                None => "another caffeine-daemon is already running".to_owned(),
            }
            .into());
        }

        file.set_len(0)?;
        file.seek(SeekFrom::Start(0))?;
        writeln!(file, "{}", std::process::id())?;
        Ok(Self { _file: file })
    }
}

pub fn path() -> PathBuf {
    std::env::temp_dir().join("caffeine-daemon.lock")
}

fn holder(file: &mut File) -> Option<u32> {
    let mut raw = String::new();
    file.seek(SeekFrom::Start(0)).ok()?;
    file.read_to_string(&mut raw).ok()?;
    raw.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_second_lock_on_a_path_is_refused() {
        let path = std::env::temp_dir().join(format!("caffeine-lock-{}", std::process::id()));
        let first = Lock::acquire_at(&path).unwrap();

        let err = Lock::acquire_at(&path).unwrap_err().to_string();
        assert!(err.contains("already running"), "{err}");
        assert!(err.contains(&std::process::id().to_string()), "{err}");

        drop(first);
        assert!(Lock::acquire_at(&path).is_ok());
        std::fs::remove_file(&path).ok();
    }
}
