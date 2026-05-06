/// Kernel TUN device transport — requires root.
use std::io;
use std::net::Ipv4Addr;
use std::os::fd::AsRawFd;

use tokio::io::Interest;
use tokio::io::unix::AsyncFd;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::ip_transport::IpTransport;
use crate::tun::{self, TunDevice};

struct RawFdWrapper(std::os::fd::RawFd);

impl AsRawFd for RawFdWrapper {
    fn as_raw_fd(&self) -> std::os::fd::RawFd {
        self.0
    }
}

pub struct TunTransport {
    nat_interface: String,
    tun: Option<TunDevice>,
    recv_task: Option<JoinHandle<()>>,
}

impl TunTransport {
    pub fn new(nat_interface: String) -> Self {
        Self {
            nat_interface,
            tun: None,
            recv_task: None,
        }
    }
}

impl IpTransport for TunTransport {
    fn setup(
        &mut self,
        local_ip: Ipv4Addr,
        peer_ip: Ipv4Addr,
        to_mobile_tx: mpsc::Sender<Vec<u8>>,
    ) -> io::Result<String> {
        let tun = TunDevice::create(local_ip, peer_ip)?;
        let name = tun.name().to_string();

        if let Err(e) = tun::enable_nat(peer_ip, tun.name(), &self.nat_interface) {
            log::warn!("TunTransport: NAT setup failed: {}", e);
        }
        tun.set_nonblocking()?;

        // Spawn read task
        let raw_fd = tun.raw_fd();
        self.tun = Some(tun);

        let recv_task = tokio::spawn(async move {
            let Ok(async_fd) = AsyncFd::with_interest(RawFdWrapper(raw_fd), Interest::READABLE)
            else {
                log::warn!("TunTransport: AsyncFd creation failed");
                return;
            };
            let mut buf = [0u8; 2048];
            loop {
                let Ok(mut guard) = async_fd.readable().await else {
                    break;
                };
                // SAFETY: we read from the TUN fd which is valid as long as self.tun exists
                let result = unsafe {
                    let ptr = buf.as_mut_ptr();
                    let n = libc::read(raw_fd, ptr as *mut libc::c_void, buf.len());
                    if n < 0 {
                        Err(io::Error::last_os_error())
                    } else {
                        Ok(n as usize)
                    }
                };
                match result {
                    Ok(0) => {
                        guard.clear_ready();
                    }
                    Ok(n) => {
                        guard.clear_ready();
                        // Platform-specific: strip AF header on macOS
                        let packet = &buf[..n];
                        #[cfg(target_os = "macos")]
                        let packet = if n > 4 { &buf[4..n] } else { packet };
                        if to_mobile_tx.send(packet.to_vec()).await.is_err() {
                            break;
                        }
                    }
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                        guard.clear_ready();
                    }
                    Err(_) => {
                        guard.clear_ready();
                    }
                }
            }
        });
        self.recv_task = Some(recv_task);

        Ok(name)
    }

    fn send_to_network(&self, ip_packet: &[u8]) -> io::Result<()> {
        if let Some(ref tun) = self.tun {
            tun.write_packet(ip_packet).map(|_| ())
        } else {
            Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "TUN not set up",
            ))
        }
    }

    fn teardown(&mut self) {
        if let Some(handle) = self.recv_task.take() {
            handle.abort();
        }
        if let Some(ref tun) = self.tun {
            let _ = tun::disable_nat(tun.peer_ip());
        }
        self.tun = None;
    }
}
