use std::pin::Pin;
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;
#[cfg(unix)]
use tokio::net::UnixStream;

pub(crate) enum LocalStream {
    Tcp(TcpStream),
    #[cfg(unix)]
    Unix(UnixStream),
}

impl AsyncRead for LocalStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            Self::Tcp(stream) => Pin::new(stream).poll_read(cx, buf),
            #[cfg(unix)]
            Self::Unix(stream) => Pin::new(stream).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for LocalStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        match self.get_mut() {
            Self::Tcp(stream) => Pin::new(stream).poll_write(cx, buf),
            #[cfg(unix)]
            Self::Unix(stream) => Pin::new(stream).poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            Self::Tcp(stream) => Pin::new(stream).poll_flush(cx),
            #[cfg(unix)]
            Self::Unix(stream) => Pin::new(stream).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            Self::Tcp(stream) => Pin::new(stream).poll_shutdown(cx),
            #[cfg(unix)]
            Self::Unix(stream) => Pin::new(stream).poll_shutdown(cx),
        }
    }
}

/// Private host endpoints are always on this machine. On Windows a refused
/// loopback SYN is retried by default, delaying every stale record by seconds.
/// Avoid those retries on this socket only; Info/authentication and operation
/// deadlines remain unchanged, and an unreachable host remains unknown.
pub(crate) async fn connect_loopback(port: u16) -> std::io::Result<TcpStream> {
    let address = std::net::SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, port));
    #[cfg(not(windows))]
    return TcpStream::connect(address).await;
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawSocket;
        use windows_sys::Win32::Networking::WinSock::{
            SIO_TCP_INITIAL_RTO, TCP_INITIAL_RTO_DEFAULT_RTT,
            TCP_INITIAL_RTO_NO_SYN_RETRANSMISSIONS, TCP_INITIAL_RTO_PARAMETERS, WSAIoctl,
        };
        let socket = tokio::net::TcpSocket::new_v4()?;
        let parameters = TCP_INITIAL_RTO_PARAMETERS {
            Rtt: TCP_INITIAL_RTO_DEFAULT_RTT as u16,
            MaxSynRetransmissions: TCP_INITIAL_RTO_NO_SYN_RETRANSMISSIONS as u8,
        };
        let mut returned = 0;
        // SAFETY: the socket is owned and open, input is the documented struct,
        // and this synchronous ioctl writes only the returned-byte count.
        // Unsupported stacks keep their default connection behavior.
        unsafe {
            WSAIoctl(
                socket.as_raw_socket() as _,
                SIO_TCP_INITIAL_RTO,
                (&parameters as *const TCP_INITIAL_RTO_PARAMETERS).cast(),
                std::mem::size_of_val(&parameters) as u32,
                std::ptr::null_mut(),
                0,
                &mut returned,
                std::ptr::null_mut(),
                None,
            );
        }
        socket.connect(address).await
    }
}
