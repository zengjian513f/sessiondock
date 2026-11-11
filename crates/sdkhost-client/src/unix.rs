pub use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::{
    ffi::{OsStrExt, OsStringExt},
    fs::{MetadataExt, PermissionsExt},
};
use std::{
    io,
    path::{Path, PathBuf},
};
pub fn encode(value: &std::ffi::OsStr) -> Vec<u8> {
    value.as_bytes().to_vec()
}
pub fn os(bytes: Vec<u8>) -> std::ffi::OsString {
    std::ffi::OsString::from_vec(bytes)
}
pub fn private_dir(path: &Path) -> io::Result<()> {
    match std::fs::create_dir(path) {
        Ok(()) => std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?,
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => (),
        Err(e) => return Err(e),
    }
    let m = std::fs::symlink_metadata(path)?;
    if !m.is_dir() || m.uid() != unsafe { libc::geteuid() } || m.mode() & 0o077 != 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "sdkhost directory must be private and owned by current user",
        ));
    }
    Ok(())
}
/// On Linux, resolve the socket through a directory descriptor to avoid the
/// sockaddr_un pathname length restriction on caller-owned storage paths.
pub fn socket_path(dir: &Path) -> io::Result<(PathBuf, Option<std::fs::File>)> {
    #[cfg(target_os = "linux")]
    {
        use std::os::fd::AsRawFd;
        let directory = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
            .open(dir)?;
        let path = PathBuf::from(format!(
            "/proc/self/fd/{}/control.sock",
            directory.as_raw_fd()
        ));
        Ok((path, Some(directory)))
    }
    #[cfg(not(target_os = "linux"))]
    {
        Ok((dir.join("control.sock"), None))
    }
}
pub fn private_dir_existing(path: &Path) -> io::Result<()> {
    let m = std::fs::symlink_metadata(path)?;
    if !m.is_dir() || m.uid() != unsafe { libc::geteuid() } || m.mode() & 0o077 != 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "invalid sdkhost directory",
        ));
    }
    Ok(())
}

pub fn lock(path: &Path) -> io::Result<std::fs::File> {
    use std::os::fd::AsRawFd;
    let f = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(f)
}
pub fn detach(cmd: &mut tokio::process::Command) {
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
}
#[cfg(target_os = "macos")]
fn relative_socket<T>(dir: &Path, action: impl FnOnce() -> io::Result<T>) -> io::Result<T> {
    use std::os::fd::AsRawFd;
    unsafe extern "C" {
        fn pthread_fchdir_np(fd: libc::c_int) -> libc::c_int;
    }
    let directory = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(dir)?;
    if unsafe { pthread_fchdir_np(directory.as_raw_fd()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    struct Restore;
    impl Drop for Restore {
        fn drop(&mut self) {
            unsafe {
                pthread_fchdir_np(-1);
            }
        }
    }
    let _restore = Restore;
    action()
}
pub async fn connect(dir: &Path) -> io::Result<tokio::net::UnixStream> {
    #[cfg(target_os = "macos")]
    let socket = {
        let socket = relative_socket(dir, || {
            std::os::unix::net::UnixStream::connect("control.sock")
        })?;
        socket.set_nonblocking(true)?;
        tokio::net::UnixStream::from_std(socket)?
    };
    #[cfg(not(target_os = "macos"))]
    let socket = {
        let (path, _directory) = socket_path(dir)?;
        tokio::net::UnixStream::connect(path).await?
    };
    if socket.peer_cred()?.uid() != unsafe { libc::geteuid() } {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "sdkhost peer owner mismatch",
        ));
    }
    Ok(socket)
}
pub struct Listener(tokio::net::UnixListener);
impl Listener {
    pub fn bind(dir: &Path) -> io::Result<Self> {
        #[cfg(target_os = "macos")]
        let listener = {
            let listener = relative_socket(dir, || {
                std::os::unix::net::UnixListener::bind("control.sock")
            })?;
            listener.set_nonblocking(true)?;
            tokio::net::UnixListener::from_std(listener)?
        };
        #[cfg(not(target_os = "macos"))]
        let listener = {
            let (path, _directory) = socket_path(dir)?;
            tokio::net::UnixListener::bind(path)?
        };
        Ok(Self(listener))
    }
    pub async fn accept(&mut self) -> io::Result<tokio::net::UnixStream> {
        loop {
            let (socket, _) = self.0.accept().await?;
            if socket.peer_cred()?.uid() == unsafe { libc::geteuid() } {
                return Ok(socket);
            }
        }
    }
}
pub fn replace(from: &Path, to: &Path) -> io::Result<()> {
    std::fs::rename(from, to)
}
