//! Local-only named pipes and current-user private storage.
use std::{
    ffi::{OsStr, OsString},
    io,
    os::windows::{
        ffi::{OsStrExt, OsStringExt},
        fs::MetadataExt,
        io::AsRawHandle,
    },
    path::Path,
};
use tokio::net::windows::named_pipe::{
    ClientOptions, NamedPipeClient, NamedPipeServer, ServerOptions,
};
use windows_sys::Win32::{
    Foundation::*,
    Security::{Authorization::*, Cryptography::*, *},
    Storage::FileSystem::*,
    System::{Pipes::*, Threading::*},
};
fn wide(s: &OsStr) -> Vec<u16> {
    s.encode_wide().chain(Some(0)).collect()
}
// Win32 path APIs need the extended namespace even when the process manifest
// or machine policy has not enabled long paths. No application path-size cap.
fn file_path(path: &Path) -> io::Result<Vec<u16>> {
    let absolute = std::path::absolute(path)?;
    let value: Vec<u16> = absolute.as_os_str().encode_wide().collect();
    let extended: Vec<u16> = if value.starts_with(&[92, 92, 63, 92]) {
        value
    } else if value.starts_with(&[92, 92]) {
        "\\\\?\\UNC\\"
            .encode_utf16()
            .chain(value.into_iter().skip(2))
            .collect()
    } else {
        "\\\\?\\".encode_utf16().chain(value).collect()
    };
    Ok(extended.into_iter().chain(Some(0)).collect())
}
pub fn encode(s: &OsStr) -> Vec<u8> {
    s.encode_wide().flat_map(u16::to_le_bytes).collect()
}
pub fn env_key_eq(encoded: &[u8], key: &OsStr) -> bool {
    let inherited: Vec<u16> = encoded
        .as_chunks::<2>()
        .0
        .iter()
        .map(|v| u16::from_le_bytes(*v))
        .collect();
    let key: Vec<u16> = key.encode_wide().collect();
    unsafe {
        windows_sys::Win32::Globalization::CompareStringOrdinal(
            inherited.as_ptr(),
            inherited.len() as i32,
            key.as_ptr(),
            key.len() as i32,
            1,
        ) == windows_sys::Win32::Globalization::CSTR_EQUAL
    }
}
pub fn os(bytes: Vec<u8>) -> OsString {
    OsString::from_wide(
        &bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|v| u16::from_le_bytes([v[0], v[1]]))
            .collect::<Vec<_>>(),
    )
}
// Files inherit the checked private directory DACL; Unix mode has no Windows meaning.
pub trait OpenOptionsExt {
    fn mode(&mut self, mode: u32) -> &mut Self;
}
impl OpenOptionsExt for std::fs::OpenOptions {
    fn mode(&mut self, _: u32) -> &mut Self {
        self
    }
}
struct Local(*mut std::ffi::c_void);
impl Drop for Local {
    fn drop(&mut self) {
        unsafe {
            LocalFree(self.0);
        }
    }
}
fn current_sid() -> io::Result<Vec<usize>> {
    unsafe {
        let mut token = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut size = 0;
        GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut size);
        let mut data = vec![0usize; (size as usize).div_ceil(std::mem::size_of::<usize>())];
        let ok = GetTokenInformation(token, TokenUser, data.as_mut_ptr().cast(), size, &mut size);
        CloseHandle(token);
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(data)
    }
}
fn security() -> io::Result<Local> {
    unsafe {
        let token = current_sid()?;
        let sid = (*(token.as_ptr() as *const TOKEN_USER)).User.Sid;
        let mut string = std::ptr::null_mut();
        if ConvertSidToStringSidW(sid, &mut string) == 0 {
            return Err(io::Error::last_os_error());
        }
        let _string = Local(string.cast());
        let mut n = 0;
        while *string.add(n) != 0 {
            n += 1;
        }
        let sid = String::from_utf16_lossy(std::slice::from_raw_parts(string, n));
        let sddl = wide(OsStr::new(&format!("O:{sid}D:P(A;OICI;FA;;;{sid})")));
        let mut sd = std::ptr::null_mut();
        if ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut sd,
            std::ptr::null_mut(),
        ) == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(Local(sd))
    }
}
pub fn private_dir(path: &Path) -> io::Result<()> {
    let sd = security()?;
    let attributes = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: sd.0,
        bInheritHandle: 0,
    };
    fn create(path: &Path, attributes: &SECURITY_ATTRIBUTES) -> io::Result<()> {
        let encoded = file_path(path)?;
        if unsafe { CreateDirectoryW(encoded.as_ptr(), attributes) } != 0 {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        match error.raw_os_error() {
            Some(code) if code == ERROR_ALREADY_EXISTS as i32 => Ok(()),
            Some(code) if code == ERROR_PATH_NOT_FOUND as i32 => {
                let parent = path.parent().filter(|p| *p != path).ok_or(error)?;
                create(parent, attributes)?;
                if unsafe { CreateDirectoryW(encoded.as_ptr(), attributes) } == 0 {
                    let error = io::Error::last_os_error();
                    if error.raw_os_error() != Some(ERROR_ALREADY_EXISTS as i32) {
                        return Err(error);
                    }
                }
                Ok(())
            }
            _ => Err(error),
        }
    }
    // Existing outer directories may be ordinary user/system paths. Only newly
    // created ancestors receive our ACL; only the requested leaf is validated.
    create(&std::path::absolute(path)?, &attributes)?;
    private_dir_existing(path)
}
pub fn private_dir_existing(path: &Path) -> io::Result<()> {
    let m = std::fs::symlink_metadata(path)?;
    if !m.is_dir() || m.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "invalid sdkhost directory",
        ));
    }
    unsafe {
        let mut owner = std::ptr::null_mut();
        let mut dacl = std::ptr::null_mut();
        let mut sd = std::ptr::null_mut();
        let error = GetNamedSecurityInfoW(
            file_path(path)?.as_ptr(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut owner,
            std::ptr::null_mut(),
            &mut dacl,
            std::ptr::null_mut(),
            &mut sd,
        );
        if error != 0 {
            return Err(io::Error::from_raw_os_error(error as i32));
        }
        let _sd = Local(sd);
        let token = current_sid()?;
        let sid = (*(token.as_ptr() as *const TOKEN_USER)).User.Sid;
        if owner.is_null() || EqualSid(owner, sid) == 0 || dacl.is_null() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "sdkhost directory owner or DACL mismatch",
            ));
        }
        for index in 0..(*dacl).AceCount {
            let mut ace = std::ptr::null_mut();
            if GetAce(dacl, index as u32, &mut ace) == 0 {
                return Err(io::Error::last_os_error());
            }
            let header = &*(ace as *const ACE_HEADER);
            if header.AceType != 0 /* ACCESS_ALLOWED_ACE_TYPE */ || EqualSid((&(*(ace as *const ACCESS_ALLOWED_ACE)).SidStart as *const u32).cast_mut().cast(), sid) == 0
            {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "sdkhost directory must grant access only to current user",
                ));
            }
        }
    }
    Ok(())
}
pub fn lock(path: &Path) -> io::Result<std::fs::File> {
    let f = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)?;
    f.lock()?;
    Ok(f)
}
pub fn replace(from: &Path, to: &Path) -> io::Result<()> {
    if unsafe {
        MoveFileExW(
            file_path(from)?.as_ptr(),
            file_path(to)?.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
fn pipe_name(dir: &Path) -> io::Result<String> {
    let name = std::fs::read_to_string(dir.join("control.pipe"))?;
    if !name.starts_with(r"\\.\pipe\sessiondock-sdkhost-")
        || !name
            .strip_prefix(r"\\.\pipe\sessiondock-sdkhost-")
            .unwrap_or("")
            .bytes()
            .all(|b| b.is_ascii_hexdigit())
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid sdkhost pipe identity",
        ));
    }
    Ok(name)
}
pub async fn connect(dir: &Path) -> io::Result<NamedPipeClient> {
    let name = pipe_name(dir)?;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    let socket = loop {
        match ClientOptions::new().open(&name) {
            Ok(s) => break s,
            Err(e)
                if e.raw_os_error() == Some(ERROR_PIPE_BUSY as i32)
                    && tokio::time::Instant::now() < deadline =>
            {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await
            }
            Err(e) => return Err(e),
        }
    };
    let mut pid = 0;
    if unsafe { GetNamedPipeServerProcessId(socket.as_raw_handle(), &mut pid) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let expected = std::fs::read_to_string(dir.join("host.pid"))?;
    if expected.trim().parse::<u32>().ok() != Some(pid) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "sdkhost pipe owner mismatch",
        ));
    }
    Ok(socket)
}
pub struct Listener {
    name: String,
    next: NamedPipeServer,
}
impl Listener {
    fn create(name: &str, first: bool) -> io::Result<NamedPipeServer> {
        let sd = security()?;
        let mut attributes = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: sd.0,
            bInheritHandle: 0,
        };
        unsafe {
            ServerOptions::new()
                .first_pipe_instance(first)
                .reject_remote_clients(true)
                .create_with_security_attributes_raw(
                    name,
                    (&mut attributes as *mut SECURITY_ATTRIBUTES).cast(),
                )
        }
    }
    pub fn bind(dir: &Path) -> io::Result<Self> {
        let mut random = [0u8; 24];
        if unsafe {
            BCryptGenRandom(
                std::ptr::null_mut(),
                random.as_mut_ptr(),
                random.len() as u32,
                BCRYPT_USE_SYSTEM_PREFERRED_RNG,
            )
        } != 0
        {
            return Err(io::Error::other("cannot create pipe identity"));
        }
        let name = format!(
            r"\\.\pipe\sessiondock-sdkhost-{}",
            random
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        );
        let next = Self::create(&name, true)?;
        std::fs::write(dir.join("control.pipe"), &name)?;
        Ok(Self { name, next })
    }
    pub async fn accept(&mut self) -> io::Result<NamedPipeServer> {
        self.next.connect().await?;
        // Keep one instance alive throughout replacement so another server cannot take the name.
        let next = Self::create(&self.name, false)?;
        Ok(std::mem::replace(&mut self.next, next))
    }
}

/// An explicit handle allow-list prevents the detached host from holding the
/// caller's inherited service/SSH pipes (or other inheritable handles) open.
pub fn spawn_host(executable: &Path, dir: &Path, spec: &[u8]) -> io::Result<()> {
    use std::{
        io::Write,
        os::windows::io::{FromRawHandle, OwnedHandle},
    };
    fn inherited(handle: HANDLE) -> io::Result<OwnedHandle> {
        let mut copy = std::ptr::null_mut();
        if unsafe {
            DuplicateHandle(
                GetCurrentProcess(),
                handle,
                GetCurrentProcess(),
                &mut copy,
                0,
                1,
                DUPLICATE_SAME_ACCESS,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(unsafe { OwnedHandle::from_raw_handle(copy) })
    }
    fn quote(value: &OsStr) -> Vec<u16> {
        let mut out = vec![34];
        let mut slashes = 0;
        for c in value.encode_wide() {
            if c == 92 {
                slashes += 1;
                continue;
            }
            out.extend(std::iter::repeat_n(
                92,
                if c == 34 { slashes * 2 + 1 } else { slashes },
            ));
            slashes = 0;
            out.push(c);
        }
        out.extend(std::iter::repeat_n(92, slashes * 2));
        out.push(34);
        out
    }
    unsafe {
        let mut read = std::ptr::null_mut();
        let mut write = std::ptr::null_mut();
        if CreatePipe(&mut read, &mut write, std::ptr::null(), 0) == 0 {
            return Err(io::Error::last_os_error());
        }
        let read = OwnedHandle::from_raw_handle(read);
        let mut write = std::fs::File::from_raw_handle(write);
        let input = inherited(read.as_raw_handle())?;
        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("host.log"))?;
        let error = inherited(log.as_raw_handle())?;
        let null = std::fs::OpenOptions::new().write(true).open("NUL")?;
        let output = inherited(null.as_raw_handle())?;
        let handles = [
            input.as_raw_handle(),
            output.as_raw_handle(),
            error.as_raw_handle(),
        ];
        let mut size = 0;
        InitializeProcThreadAttributeList(std::ptr::null_mut(), 1, 0, &mut size);
        let mut storage = vec![0usize; size.div_ceil(std::mem::size_of::<usize>())];
        let attributes = storage.as_mut_ptr().cast();
        if InitializeProcThreadAttributeList(attributes, 1, 0, &mut size) == 0 {
            return Err(io::Error::last_os_error());
        }
        struct Attributes(LPPROC_THREAD_ATTRIBUTE_LIST);
        impl Drop for Attributes {
            fn drop(&mut self) {
                unsafe {
                    DeleteProcThreadAttributeList(self.0);
                }
            }
        }
        let _attributes = Attributes(attributes);
        if UpdateProcThreadAttribute(
            attributes,
            0,
            PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
            handles.as_ptr().cast(),
            std::mem::size_of_val(&handles),
            std::ptr::null_mut(),
            std::ptr::null(),
        ) == 0
        {
            return Err(io::Error::last_os_error());
        }
        let mut startup: STARTUPINFOEXW = std::mem::zeroed();
        startup.StartupInfo.cb = std::mem::size_of_val(&startup) as u32;
        startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
        startup.StartupInfo.hStdInput = handles[0];
        startup.StartupInfo.hStdOutput = handles[1];
        startup.StartupInfo.hStdError = handles[2];
        startup.lpAttributeList = attributes;
        let mut command = quote(executable.as_os_str());
        command.extend(" --dir ".encode_utf16());
        command.extend(quote(dir.as_os_str()));
        command.push(0);
        let mut process: PROCESS_INFORMATION = std::mem::zeroed();
        if CreateProcessW(
            wide(executable.as_os_str()).as_ptr(),
            command.as_mut_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            1,
            EXTENDED_STARTUPINFO_PRESENT
                | DETACHED_PROCESS
                | CREATE_NEW_PROCESS_GROUP
                | CREATE_BREAKAWAY_FROM_JOB,
            std::ptr::null(),
            std::ptr::null(),
            &startup.StartupInfo,
            &mut process,
        ) == 0
        {
            let e = io::Error::last_os_error();
            // No process was created; this new instance has no durable identity yet.
            let _ = std::fs::remove_dir_all(dir);
            return Err(e);
        }
        CloseHandle(process.hThread);
        CloseHandle(process.hProcess);
        drop(input);
        drop(read);
        write.write_all(spec)?;
    }
    Ok(())
}
