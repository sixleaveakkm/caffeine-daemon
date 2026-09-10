use std::error::Error;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// The launchd job name, and the plist's file name under `~/Library/LaunchAgents`.
pub const LABEL: &str = "local.caffeine-daemon";

const STDOUT_PATH: &str = "/tmp/caffeine-daemon.log";
const STDERR_PATH: &str = "/tmp/caffeine-daemon.err";

/// `~/Library/LaunchAgents/local.caffeine-daemon.plist`.
pub fn path() -> Result<PathBuf, Box<dyn Error>> {
    let home = std::env::var_os("HOME").ok_or("HOME is not set")?;
    Ok(Path::new(&home)
        .join("Library/LaunchAgents")
        .join(format!("{LABEL}.plist")))
}

/// What the plist should point `ProgramArguments` at. The copy on `PATH` wins
/// over the one being run, so generating this from a build directory still
/// names the installed binary.
pub fn program() -> Result<PathBuf, Box<dyn Error>> {
    let running = std::env::current_exe()?;
    let name = running
        .file_name()
        .ok_or("cannot tell what this binary is called")?;
    Ok(on_path(name).unwrap_or(running))
}

fn on_path(name: &OsStr) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join(name))
        .find(|candidate| is_executable(candidate))
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

/// A LaunchAgent, not a LaunchDaemon: `caffeinate` only holds the machine awake
/// from inside the logged-in GUI session.
pub fn plist(program: &Path) -> String {
    let program = escape(&program.to_string_lossy());
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{LABEL}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{program}</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>{STDOUT_PATH}</string>
    <key>StandardErrorPath</key>
    <string>{STDERR_PATH}</string>
</dict>
</plist>
"#
    )
}

/// A path is free to hold `&` or `<`, which would end the plist's `<string>`
/// somewhere launchd does not expect.
fn escape(raw: &str) -> String {
    raw.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_program_path_is_what_launchd_runs() {
        let plist = plist(Path::new("/usr/local/bin/caffeine-daemon"));
        assert!(plist.contains("<string>/usr/local/bin/caffeine-daemon</string>"));
        assert!(plist.contains(&format!("<string>{LABEL}</string>")));
        assert!(plist.ends_with("</plist>\n"));
    }

    #[test]
    fn a_path_cannot_break_out_of_its_string() {
        let plist = plist(Path::new("/Users/me/bin & tools/caffeine-daemon"));
        assert!(plist.contains("/Users/me/bin &amp; tools/caffeine-daemon"));
        assert_eq!(plist.matches("<string>").count(), 4);
    }

    #[test]
    fn only_an_absolute_executable_counts_as_on_path() {
        assert!(!is_executable(Path::new("/definitely/not/here")));
        assert!(is_executable(Path::new("/bin/sh")));
        // A directory named like the binary is not the binary.
        assert!(!is_executable(Path::new("/tmp")));
    }
}
