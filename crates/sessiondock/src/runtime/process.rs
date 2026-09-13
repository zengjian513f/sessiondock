//! Process identity evidence for host-managed instances.
//!
//! Linux reads only the `/proc/<pid>/stat` entries named by an already verified
//! host record (its child and the host itself), plus `/proc/stat` and
//! `/proc/self/auxv` for the boot clock. Nothing enumerates `/proc`, sends a
//! signal, or reads a process owned by another user.
//!
//! Windows opens the named PID with `PROCESS_QUERY_LIMITED_INFORMATION` only
//! and takes the kernel's creation `FILETIME` from `GetProcessTimes` as the
//! start time; the owner check compares the process token's user SID with this
//! service's own. Nothing enumerates the process table, and an exited process
//! whose PID is still reserved by an open handle (our own retained `Child`
//! included) reads as `not_visible`, exactly like a vanished `/proc/<pid>`.
//!
//! Other platforms return a typed `unsupported_platform` reason: absence of a
//! process table is never an exit.

use serde::Serialize;

/// Kernel start time in platform ticks: Linux clock ticks after boot
/// (`/proc/<pid>/stat` field 22), Windows 100 ns `FILETIME` units after the
/// Unix epoch (`GetProcessTimes` creation time). Exact for the life of the
/// process. Identities are compared in ticks, never in converted wall-clock
/// seconds: the reported boot timestamp itself can move by a second under
/// clock adjustments while the ticks of a live process never change.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct StartTime(pub u64);

/// One process incarnation. A reused PID has a different start time and is
/// therefore a different process, whatever a stale record claims.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct ProcessIdentity {
    pub pid: u32,
    pub start_time: StartTime,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentityFailure {
    /// No process table support on this target; nothing is inferred.
    UnsupportedPlatform,
    /// `/proc/<pid>` does not exist, or the Windows PID names no running
    /// process. On its own this is not proof of exit.
    NotVisible,
    /// The process exists but could not be read (or opened).
    Unreadable,
    /// The process belongs to another user; it is never read.
    NotOwned,
    /// The stat line did not parse, names a different PID, or the PID is 0.
    Malformed,
    /// The PID now carries a different start time: a different process.
    Mismatch,
    /// The PID started after the host record was written: a reused PID.
    StartedAfterRecord,
}

impl ProcessIdentity {
    /// Parse one `/proc/<pid>/stat` line. `comm` may contain spaces and
    /// parentheses, so fields are located after the last `)`.
    pub fn parse_stat(expected_pid: u32, stat: &str) -> Result<Self, IdentityFailure> {
        let open = stat.find(" (").ok_or(IdentityFailure::Malformed)?;
        let pid: u32 = stat[..open]
            .trim()
            .parse()
            .map_err(|_| IdentityFailure::Malformed)?;
        if pid != expected_pid {
            return Err(IdentityFailure::Malformed);
        }
        let close = stat.rfind(')').ok_or(IdentityFailure::Malformed)?;
        if close < open {
            return Err(IdentityFailure::Malformed);
        }
        // Field 3 (state) is the first token after `)`; field 22 is starttime.
        let start = stat[close + 1..]
            .split_ascii_whitespace()
            .nth(19)
            .ok_or(IdentityFailure::Malformed)?
            .parse::<u64>()
            .map_err(|_| IdentityFailure::Malformed)?;
        Ok(Self {
            pid,
            start_time: StartTime(start),
        })
    }

    /// Exact comparison against a previously captured incarnation.
    pub fn verify(&self, observed: &Self) -> Result<(), IdentityFailure> {
        if self == observed {
            Ok(())
        } else {
            Err(IdentityFailure::Mismatch)
        }
    }
}

/// Clock for turning start ticks into legacy Unix seconds: Linux ticks are
/// counted from `boot_time` at `AT_CLKTCK` per second; Windows ticks are
/// counted from the Unix epoch itself (`boot_time` 0) at `FILETIME_PER_SECOND`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProcClock {
    pub boot_time: u64,
    pub ticks_per_second: u64,
}

impl ProcClock {
    pub const AT_CLKTCK: usize = 17;
    pub const DEFAULT_TICKS: u64 = 100;
    /// 100 ns `FILETIME` units per second.
    pub const FILETIME_PER_SECOND: u64 = 10_000_000;

    pub fn unix_seconds(&self, start: StartTime) -> f64 {
        let seconds = self.boot_time as f64 + start.0 as f64 / self.ticks_per_second.max(1) as f64;
        (seconds * 100.0).round() / 100.0
    }

    /// `btime` from `/proc/stat`.
    pub fn parse_boot_time(proc_stat: &str) -> Option<u64> {
        proc_stat
            .lines()
            .find_map(|line| line.strip_prefix("btime "))
            .and_then(|value| value.trim().parse().ok())
    }

    /// `AT_CLKTCK` from the ELF auxiliary vector of this process, the same
    /// source glibc's `sysconf(_SC_CLK_TCK)` uses. No entry means the default.
    pub fn parse_auxv(bytes: &[u8]) -> u64 {
        let width = std::mem::size_of::<usize>();
        bytes
            .chunks_exact(width * 2)
            .map(|pair| {
                let key = usize::from_ne_bytes(pair[..width].try_into().expect("width"));
                let value = usize::from_ne_bytes(pair[width..].try_into().expect("width"));
                (key, value)
            })
            .take_while(|(key, _)| *key != 0)
            .find(|(key, _)| *key == Self::AT_CLKTCK)
            .map(|(_, value)| value as u64)
            .filter(|value| *value > 0)
            .unwrap_or(Self::DEFAULT_TICKS)
    }

    /// Latest start (in ticks) a process may have if it was already running
    /// when a record stamped `created` (Unix seconds). Second rounding and boot
    /// clock jitter get a fixed slack; a later start means a reused PID.
    pub fn latest_start_for(&self, created: u64, slack_seconds: u64) -> StartTime {
        let seconds = (created + slack_seconds).saturating_sub(self.boot_time);
        StartTime(seconds.saturating_mul(self.ticks_per_second))
    }
}

#[cfg(target_os = "linux")]
pub const PLATFORM: &str = "linux_proc";
#[cfg(windows)]
pub const PLATFORM: &str = "windows_process_times";
#[cfg(not(any(target_os = "linux", windows)))]
pub const PLATFORM: &str = "unsupported";

#[cfg(target_os = "linux")]
pub async fn load_clock() -> Option<ProcClock> {
    let stat = tokio::fs::read_to_string("/proc/stat").await.ok()?;
    let boot_time = ProcClock::parse_boot_time(&stat)?;
    let ticks_per_second = match tokio::fs::read("/proc/self/auxv").await {
        Ok(bytes) => ProcClock::parse_auxv(&bytes),
        Err(_) => ProcClock::DEFAULT_TICKS,
    };
    Some(ProcClock {
        boot_time,
        ticks_per_second,
    })
}

/// Windows start times are already epoch-relative `FILETIME` ticks.
#[cfg(windows)]
pub async fn load_clock() -> Option<ProcClock> {
    Some(ProcClock {
        boot_time: 0,
        ticks_per_second: ProcClock::FILETIME_PER_SECOND,
    })
}

#[cfg(not(any(target_os = "linux", windows)))]
pub async fn load_clock() -> Option<ProcClock> {
    None
}

/// Read one named process. `check_owner` additionally refuses a process whose
/// `/proc/<pid>` directory is owned by a different user than this service.
#[cfg(target_os = "linux")]
pub async fn observe(pid: u32, check_owner: bool) -> Result<ProcessIdentity, IdentityFailure> {
    use std::os::unix::fs::MetadataExt;

    if pid == 0 {
        return Err(IdentityFailure::Malformed);
    }
    let directory = format!("/proc/{pid}");
    if check_owner {
        let own = tokio::fs::metadata("/proc/self")
            .await
            .map_err(|_| IdentityFailure::Unreadable)?
            .uid();
        let owner = tokio::fs::metadata(&directory)
            .await
            .map_err(io_failure)?
            .uid();
        if owner != own {
            return Err(IdentityFailure::NotOwned);
        }
    }
    let stat = tokio::fs::read_to_string(format!("{directory}/stat"))
        .await
        .map_err(io_failure)?;
    ProcessIdentity::parse_stat(pid, &stat)
}

#[cfg(target_os = "linux")]
fn io_failure(error: std::io::Error) -> IdentityFailure {
    if error.kind() == std::io::ErrorKind::NotFound {
        IdentityFailure::NotVisible
    } else {
        IdentityFailure::Unreadable
    }
}

/// Read one named process through a query-only handle. `check_owner`
/// additionally refuses a process whose token user SID differs from ours; a
/// token that cannot be opened or compared is refused too, never passed.
#[cfg(windows)]
pub async fn observe(pid: u32, check_owner: bool) -> Result<ProcessIdentity, IdentityFailure> {
    // Four fast kernel queries on one handle; nothing here blocks or waits.
    windows::observe(pid, check_owner)
}

#[cfg(not(any(target_os = "linux", windows)))]
pub async fn observe(_pid: u32, _check_owner: bool) -> Result<ProcessIdentity, IdentityFailure> {
    Err(IdentityFailure::UnsupportedPlatform)
}

#[cfg(windows)]
mod windows {
    use super::{IdentityFailure, ProcessIdentity, StartTime};
    use std::ptr;
    use windows_sys::Win32::{
        Foundation::{
            CloseHandle, ERROR_ACCESS_DENIED, ERROR_INVALID_PARAMETER, FILETIME, GetLastError,
            HANDLE, STILL_ACTIVE,
        },
        Security::{EqualSid, GetTokenInformation, PSID, TOKEN_QUERY, TOKEN_USER, TokenUser},
        System::Threading::{
            GetCurrentProcess, GetExitCodeProcess, GetProcessTimes, OpenProcess, OpenProcessToken,
            PROCESS_QUERY_LIMITED_INFORMATION,
        },
    };

    /// 100 ns intervals between 1601-01-01 (`FILETIME` zero) and 1970-01-01.
    const EPOCH_OFFSET: u64 = 116_444_736_000_000_000;

    struct Handle(HANDLE);
    impl Drop for Handle {
        fn drop(&mut self) {
            // SAFETY: the handle was returned open by OpenProcess or
            // OpenProcessToken and is closed exactly once here.
            unsafe { CloseHandle(self.0) };
        }
    }

    /// `TOKEN_USER` plus its trailing SID, kept alive for `EqualSid`.
    struct TokenUserBuffer(Vec<u64>);
    impl TokenUserBuffer {
        fn read(token: HANDLE) -> Result<Self, IdentityFailure> {
            let mut needed = 0u32;
            // SAFETY: a zero-length query only reports the required size.
            unsafe { GetTokenInformation(token, TokenUser, ptr::null_mut(), 0, &mut needed) };
            let words = (needed as usize).div_ceil(std::mem::size_of::<u64>());
            if needed == 0 || words == 0 || needed > 64 * 1024 {
                return Err(IdentityFailure::Unreadable);
            }
            let mut buffer = vec![0u64; words];
            let mut written = 0u32;
            // SAFETY: the buffer is 8-byte aligned (TOKEN_USER holds a
            // pointer) and at least `needed` bytes long.
            let ok = unsafe {
                GetTokenInformation(
                    token,
                    TokenUser,
                    buffer.as_mut_ptr().cast(),
                    needed,
                    &mut written,
                )
            };
            if ok == 0 || written as usize > words * std::mem::size_of::<u64>() {
                return Err(IdentityFailure::Unreadable);
            }
            Ok(Self(buffer))
        }
        fn sid(&self) -> PSID {
            // SAFETY: `read` filled the buffer with a TOKEN_USER whose Sid
            // pointer refers into this same buffer, which is still alive.
            unsafe { (*self.0.as_ptr().cast::<TOKEN_USER>()).User.Sid }
        }
    }

    fn token_user(
        process: HANDLE,
        denied: IdentityFailure,
    ) -> Result<TokenUserBuffer, IdentityFailure> {
        let mut token: HANDLE = ptr::null_mut();
        // SAFETY: `process` is an open handle; `token` receives a new handle.
        if unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) } == 0 {
            // SAFETY: plain thread-local error read.
            return Err(if unsafe { GetLastError() } == ERROR_ACCESS_DENIED {
                denied
            } else {
                IdentityFailure::Unreadable
            });
        }
        let token = Handle(token);
        TokenUserBuffer::read(token.0)
    }

    fn filetime(value: FILETIME) -> u64 {
        (u64::from(value.dwHighDateTime) << 32) | u64::from(value.dwLowDateTime)
    }

    pub(super) fn observe(pid: u32, check_owner: bool) -> Result<ProcessIdentity, IdentityFailure> {
        if pid == 0 {
            return Err(IdentityFailure::Malformed);
        }
        // SAFETY: plain Win32 call; a null result is checked below.
        let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if process.is_null() {
            // SAFETY: plain thread-local error read.
            return Err(match unsafe { GetLastError() } {
                // The documented result for a PID that names no process.
                ERROR_INVALID_PARAMETER => IdentityFailure::NotVisible,
                _ => IdentityFailure::Unreadable,
            });
        }
        let process = Handle(process);
        let mut code = 0u32;
        // SAFETY: open handle with query rights; `code` receives the status.
        if unsafe { GetExitCodeProcess(process.0, &mut code) } == 0 {
            return Err(IdentityFailure::Unreadable);
        }
        if code != STILL_ACTIVE as u32 {
            // Exited, PID merely reserved by an open handle: as gone as a
            // vanished /proc entry.
            return Err(IdentityFailure::NotVisible);
        }
        if check_owner {
            let theirs = token_user(process.0, IdentityFailure::NotOwned)?;
            // SAFETY: the pseudo-handle needs no closing; token_user only
            // opens a token from it.
            let ours = token_user(unsafe { GetCurrentProcess() }, IdentityFailure::Unreadable)?;
            // SAFETY: both SIDs point into buffers that outlive this call.
            if unsafe { EqualSid(theirs.sid(), ours.sid()) } == 0 {
                return Err(IdentityFailure::NotOwned);
            }
        }
        let zero = FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        };
        let (mut creation, mut exit, mut kernel, mut user) = (zero, zero, zero, zero);
        // SAFETY: open handle with query rights; all four out-params are valid.
        if unsafe { GetProcessTimes(process.0, &mut creation, &mut exit, &mut kernel, &mut user) }
            == 0
        {
            return Err(IdentityFailure::Unreadable);
        }
        let start = filetime(creation)
            .checked_sub(EPOCH_OFFSET)
            .ok_or(IdentityFailure::Malformed)?;
        Ok(ProcessIdentity {
            pid,
            start_time: StartTime(start),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STAT: &str = "4242 (sleep) S 1 4242 4242 0 -1 4194560 100 0 0 0 0 0 0 0 20 0 1 0 123456 5000000 100 18446744073709551615 1 1 0 0 0 0 0 0 0 0 0 0 17 3 0 0 0 0 0 0 0 0 0 0 0 0 0";

    #[test]
    fn stat_parser_reads_field_22_after_a_comm_with_spaces_and_parentheses() {
        let identity = ProcessIdentity::parse_stat(4242, STAT).unwrap();
        assert_eq!(identity.pid, 4242);
        assert_eq!(identity.start_time, StartTime(123456));
        let odd = STAT.replacen("(sleep)", "(a b) (c) d)", 1);
        assert_eq!(ProcessIdentity::parse_stat(4242, &odd).unwrap(), identity);
        for malformed in [
            "",
            "4242",
            "4242 (sleep",
            "4242 (sleep) S 1",
            "abc (sleep) S 1 4242",
            &STAT.replacen("123456", "x", 1),
        ] {
            assert_eq!(
                ProcessIdentity::parse_stat(4242, malformed).unwrap_err(),
                IdentityFailure::Malformed,
                "{malformed:?}"
            );
        }
        assert_eq!(
            ProcessIdentity::parse_stat(4243, STAT).unwrap_err(),
            IdentityFailure::Malformed
        );
    }

    #[test]
    fn a_reused_pid_with_a_different_start_time_is_a_different_process() {
        let captured = ProcessIdentity::parse_stat(4242, STAT).unwrap();
        let reused =
            ProcessIdentity::parse_stat(4242, &STAT.replacen("123456", "123457", 1)).unwrap();
        assert_eq!(captured.pid, reused.pid);
        assert_eq!(captured.verify(&reused), Err(IdentityFailure::Mismatch));
        assert_eq!(captured.verify(&captured), Ok(()));
    }

    #[test]
    fn boot_clock_parses_btime_auxv_and_converts_ticks() {
        let stat = "cpu  1 2 3 4\ncpu0 1 2 3 4\nintr 0\nctxt 9\nbtime 1700000000\nprocesses 1\n";
        assert_eq!(ProcClock::parse_boot_time(stat), Some(1_700_000_000));
        assert_eq!(ProcClock::parse_boot_time("cpu 1\nbtime x\n"), None);
        assert_eq!(ProcClock::parse_boot_time(""), None);
        let mut auxv = Vec::new();
        for (key, value) in [
            (33usize, 7usize),
            (ProcClock::AT_CLKTCK, 250),
            (0, 0),
            (17, 1),
        ] {
            auxv.extend_from_slice(&key.to_ne_bytes());
            auxv.extend_from_slice(&value.to_ne_bytes());
        }
        assert_eq!(ProcClock::parse_auxv(&auxv), 250);
        assert_eq!(ProcClock::parse_auxv(&[]), ProcClock::DEFAULT_TICKS);
        assert_eq!(ProcClock::parse_auxv(&auxv[..3]), ProcClock::DEFAULT_TICKS);
        let clock = ProcClock {
            boot_time: 1_700_000_000,
            ticks_per_second: 100,
        };
        assert_eq!(clock.unix_seconds(StartTime(123456)), 1_700_001_234.56);
        assert_eq!(clock.latest_start_for(1_700_000_010, 5), StartTime(1500));
        assert_eq!(clock.latest_start_for(1, 5), StartTime(0));
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn linux_reads_only_the_named_owned_child_and_notices_its_exit() {
        use std::process::{Command, Stdio};
        let mut child = Command::new("sleep")
            .arg("30")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let pid = child.id();
        let clock = load_clock().await.unwrap();
        assert!(clock.ticks_per_second > 0 && clock.boot_time > 0);
        let captured = observe(pid, true).await.unwrap();
        assert_eq!(captured.pid, pid);
        let again = observe(pid, false).await.unwrap();
        assert_eq!(captured.verify(&again), Ok(()));
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let started = clock.unix_seconds(captured.start_time);
        assert!(started <= now as f64 + 5.0 && started >= now as f64 - 120.0);
        assert!(captured.start_time <= clock.latest_start_for(now, 5));
        assert_eq!(
            observe(std::process::id(), true).await.unwrap().pid,
            std::process::id()
        );
        assert_eq!(observe(0, true).await, Err(IdentityFailure::Malformed));
        // PID 1 belongs to root: an unprivileged service refuses to read it.
        use std::os::unix::fs::MetadataExt;
        if tokio::fs::metadata("/proc/self").await.unwrap().uid() != 0
            && tokio::fs::metadata("/proc/1").await.is_ok()
        {
            assert_eq!(observe(1, true).await, Err(IdentityFailure::NotOwned));
        }
        child.kill().unwrap();
        child.wait().unwrap();
        assert_eq!(observe(pid, false).await, Err(IdentityFailure::NotVisible));
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn windows_reads_only_the_named_owned_child_and_notices_its_exit() {
        use std::process::{Command, Stdio};
        let mut child = Command::new("cmd.exe")
            .args(["/d", "/c", "pause"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let pid = child.id();
        let clock = load_clock().await.unwrap();
        assert_eq!(clock.ticks_per_second, ProcClock::FILETIME_PER_SECOND);
        let captured = observe(pid, true).await.unwrap();
        assert_eq!(captured.pid, pid);
        let again = observe(pid, false).await.unwrap();
        assert_eq!(captured.verify(&again), Ok(()));
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let started = clock.unix_seconds(captured.start_time);
        assert!(started <= now as f64 + 5.0 && started >= now as f64 - 120.0);
        assert!(captured.start_time <= clock.latest_start_for(now, 5));
        assert!(captured.start_time > clock.latest_start_for(now - 120, 0));
        assert_eq!(
            observe(std::process::id(), true).await.unwrap().pid,
            std::process::id()
        );
        assert_eq!(observe(0, true).await, Err(IdentityFailure::Malformed));
        // PID 4 is the System process: another user's token, never accepted.
        assert!(matches!(
            observe(4, true).await,
            Err(IdentityFailure::NotOwned | IdentityFailure::Unreadable)
        ));
        child.kill().unwrap();
        // The retained Child handle keeps the PID reserved: exit must still show.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            match observe(pid, false).await {
                Err(IdentityFailure::NotVisible) => break,
                Ok(_) if std::time::Instant::now() < deadline => {
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                }
                other => panic!("exited child still observed: {other:?}"),
            }
        }
        child.wait().unwrap();
        assert_eq!(observe(pid, false).await, Err(IdentityFailure::NotVisible));
    }
}
