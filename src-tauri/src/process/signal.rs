//! Asking a process to stop, and making it stop.
//!
//! Windows has no SIGTERM. `taskkill /PID <pid> /T` walks the process tree and
//! asks politely; `/F` is the hard kill. On Unix the same two stages are SIGTERM
//! and SIGKILL, sent to the process group so the JVM's children go too.

/// Politely asks a process (and its children) to exit.
///
/// Returns false when the request could not be delivered at all — the process
/// may already be gone, which callers treat as success.
pub fn request_terminate(pid: u32) -> bool {
    #[cfg(unix)]
    {
        // Negative pid targets the whole process group, which is why servers are
        // started with `process_group(0)`.
        unsafe { libc::kill(-(pid as i32), libc::SIGTERM) == 0 }
    }

    #[cfg(windows)]
    {
        taskkill(pid, false)
    }
}

/// Kills a process and its children outright.
pub fn force_kill(pid: u32) -> bool {
    #[cfg(unix)]
    {
        unsafe { libc::kill(-(pid as i32), libc::SIGKILL) == 0 }
    }

    #[cfg(windows)]
    {
        taskkill(pid, true)
    }
}

#[cfg(windows)]
fn taskkill(pid: u32, force: bool) -> bool {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    let mut command = std::process::Command::new("taskkill");
    command.arg("/PID").arg(pid.to_string()).arg("/T");
    if force {
        command.arg("/F");
    }
    command.creation_flags(CREATE_NO_WINDOW);

    match command.output() {
        Ok(output) => output.status.success(),
        Err(err) => {
            tracing::warn!(error = %err, pid, "could not run taskkill");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signalling_a_pid_that_does_not_exist_is_reported_not_fatal() {
        // 0xFFFF_FFF0 is not a live pid on either platform; the call must return
        // rather than panic, and must not claim success.
        assert!(!request_terminate(4_294_967_280));
        assert!(!force_kill(4_294_967_280));
    }
}
