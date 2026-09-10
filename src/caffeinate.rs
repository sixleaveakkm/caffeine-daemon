use std::io;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard};

/// Whatever keeps the machine awake, so tests and non-macOS builds can stand in
/// for the real thing.
pub trait Inhibitor: Send + Sync {
    fn engage(&self) -> io::Result<()>;
    fn release(&self) -> io::Result<()>;
    fn engaged(&self) -> bool;
    /// `None` while idle, and for adapters that hold no process of their own.
    fn pid(&self) -> Option<u32>;
}

/// Holds a `caffeinate -dis` child for as long as anyone is holding.
#[derive(Default)]
pub struct Caffeinate {
    child: Mutex<Option<Child>>,
}

impl Caffeinate {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Inhibitor for Caffeinate {
    fn engage(&self) -> io::Result<()> {
        let mut slot = lock(&self.child);
        if slot.is_some() {
            return Ok(());
        }
        // -w <our pid> makes caffeinate exit with us, so a crashed daemon
        // cannot leave the Mac awake for good.
        let child = Command::new("caffeinate")
            .args(["-dis", "-w"])
            .arg(std::process::id().to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        *slot = Some(child);
        Ok(())
    }

    fn release(&self) -> io::Result<()> {
        let mut slot = lock(&self.child);
        let Some(mut child) = slot.take() else {
            return Ok(());
        };
        child.kill()?;
        child.wait()?;
        Ok(())
    }

    fn engaged(&self) -> bool {
        lock(&self.child).is_some()
    }

    fn pid(&self) -> Option<u32> {
        lock(&self.child).as_ref().map(|child| child.id())
    }
}

/// Stands in for `caffeinate` off macOS: holds are tracked, nothing is kept awake.
#[derive(Default)]
pub struct Noop {
    engaged: AtomicBool,
}

impl Noop {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Inhibitor for Noop {
    fn engage(&self) -> io::Result<()> {
        self.engaged.store(true, Ordering::SeqCst);
        Ok(())
    }

    fn release(&self) -> io::Result<()> {
        self.engaged.store(false, Ordering::SeqCst);
        Ok(())
    }

    fn engaged(&self) -> bool {
        self.engaged.load(Ordering::SeqCst)
    }

    fn pid(&self) -> Option<u32> {
        None
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
