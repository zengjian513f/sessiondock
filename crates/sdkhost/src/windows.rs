//! Own the complete native process tree, even after its original process exits.
use std::{
    io,
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
};
use windows_sys::Win32::{
    Foundation::*,
    System::{Diagnostics::ToolHelp::*, JobObjects::*, Threading::*},
};
pub struct Job(OwnedHandle);
impl Job {
    pub fn spawn(
        command: &mut tokio::process::Command,
    ) -> io::Result<(Self, tokio::process::Child)> {
        unsafe {
            let raw = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if raw.is_null() {
                return Err(io::Error::last_os_error());
            }
            let job = Self(OwnedHandle::from_raw_handle(raw));
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if SetInformationJobObject(
                raw,
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                std::mem::size_of_val(&limits) as u32,
            ) == 0
            {
                return Err(io::Error::last_os_error());
            }
            command.creation_flags(CREATE_SUSPENDED | CREATE_NO_WINDOW);
            let mut child = command.spawn()?;
            let result = (|| {
                if AssignProcessToJobObject(raw, child.raw_handle().unwrap()) == 0 {
                    return Err(io::Error::last_os_error());
                }
                let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
                if snapshot == INVALID_HANDLE_VALUE {
                    return Err(io::Error::last_os_error());
                }
                let snapshot = OwnedHandle::from_raw_handle(snapshot);
                let mut entry: THREADENTRY32 = std::mem::zeroed();
                entry.dwSize = std::mem::size_of_val(&entry) as u32;
                let mut found = Thread32First(snapshot.as_raw_handle(), &mut entry);
                while found != 0 {
                    if entry.th32OwnerProcessID == child.id().unwrap() {
                        let thread = OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID);
                        if thread.is_null() {
                            return Err(io::Error::last_os_error());
                        }
                        let thread = OwnedHandle::from_raw_handle(thread);
                        if ResumeThread(thread.as_raw_handle()) == u32::MAX {
                            return Err(io::Error::last_os_error());
                        }
                        return Ok(());
                    }
                    found = Thread32Next(snapshot.as_raw_handle(), &mut entry);
                }
                Err(io::Error::other(
                    "suspended SDK process has no primary thread",
                ))
            })();
            if let Err(e) = result {
                let _ = child.start_kill();
                return Err(e);
            }
            Ok((job, child))
        }
    }
    pub fn signal(&self, signal: i32) -> io::Result<()> {
        if !matches!(signal, 9 | 15) {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "Windows SDK host supports stop (9 or 15), not Unix signals",
            ));
        }
        if unsafe { TerminateJobObject(self.0.as_raw_handle(), 1) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}
