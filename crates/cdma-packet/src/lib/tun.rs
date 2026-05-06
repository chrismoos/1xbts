/// TUN device management for packet data IP forwarding.
///
/// Creates a point-to-point TUN interface for each packet data session,
/// configures IP addresses, and provides read/write access to the tunnel fd.
///
/// Platform support:
/// - macOS: uses utun via SYSPROTO_CONTROL socket
/// - Linux: uses /dev/net/tun with IFF_TUN | IFF_NO_PI
use std::io;
use std::net::Ipv4Addr;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::process::Command;

fn log_command_status(action: &str, result: io::Result<std::process::ExitStatus>) {
    match result {
        Ok(status) if status.success() => {}
        Ok(status) => log::warn!("TUN: {action} exited with status {status}"),
        Err(error) => log::warn!("TUN: {action} failed: {error}"),
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn unsupported_read_packet(_buf: &mut [u8]) -> io::Result<usize> {
    Err(io::Error::new(io::ErrorKind::Unsupported, "unsupported"))
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn unsupported_write_packet(_packet: &[u8]) -> io::Result<usize> {
    Err(io::Error::new(io::ErrorKind::Unsupported, "unsupported"))
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn log_unsupported_nat(_peer_ip: Ipv4Addr, _tun_name: &str, _nat_interface: &str) {
    log::warn!("TUN: NAT not supported on this platform");
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn log_unsupported_nat_disable(_peer_ip: Ipv4Addr) {}

/// A TUN device handle. Provides read/write of raw IP packets.
pub struct TunDevice {
    fd: OwnedFd,
    name: String,
    local_ip: Ipv4Addr,
    peer_ip: Ipv4Addr,
}

impl TunDevice {
    /// Create and configure a TUN device.
    ///
    /// - `local_ip`: our (gateway) side of the point-to-point link
    /// - `peer_ip`: the mobile's assigned IP
    ///
    /// Requires root/admin privileges.
    pub fn create(local_ip: Ipv4Addr, peer_ip: Ipv4Addr) -> io::Result<Self> {
        #[cfg(target_os = "macos")]
        let (fd, name) = create_utun()?;

        #[cfg(target_os = "linux")]
        let (fd, name) = create_tun_linux()?;

        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "TUN not supported on this platform",
        ));

        log::info!(
            "TUN: created device {} (local={}, peer={})",
            name,
            local_ip,
            peer_ip
        );

        // Configure IP addresses on the interface.
        configure_interface(&name, local_ip, peer_ip)?;

        Ok(TunDevice {
            fd,
            name,
            local_ip,
            peer_ip,
        })
    }

    /// Device name (e.g. "utun4" or "cdma-pkt0").
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn local_ip(&self) -> Ipv4Addr {
        self.local_ip
    }

    pub fn peer_ip(&self) -> Ipv4Addr {
        self.peer_ip
    }

    /// Read an IP packet from the TUN device (blocking).
    ///
    /// Returns the raw IP packet bytes (no TUN header on Linux with IFF_NO_PI;
    /// on macOS, the 4-byte AF header is stripped).
    pub fn read_packet(&self, buf: &mut [u8]) -> io::Result<usize> {
        #[cfg(target_os = "macos")]
        {
            // macOS utun prepends a 4-byte protocol header (AF_INET = 2).
            let mut tmp = vec![0u8; buf.len() + 4];
            let n = read_fd(self.fd.as_raw_fd(), &mut tmp)?;
            if n <= 4 {
                return Ok(0);
            }
            let payload_len = n - 4;
            buf[..payload_len].copy_from_slice(&tmp[4..n]);
            Ok(payload_len)
        }

        #[cfg(target_os = "linux")]
        {
            read_fd(self.fd.as_raw_fd(), buf)
        }

        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            unsupported_read_packet(buf)
        }
    }

    /// Write an IP packet to the TUN device (for delivery to the mobile's IP stack).
    pub fn write_packet(&self, packet: &[u8]) -> io::Result<usize> {
        #[cfg(target_os = "macos")]
        {
            // macOS utun requires a 4-byte AF header prefix.
            let mut tmp = Vec::with_capacity(4 + packet.len());
            tmp.extend_from_slice(&[0, 0, 0, 2]); // AF_INET
            tmp.extend_from_slice(packet);
            write_fd(self.fd.as_raw_fd(), &tmp)
        }

        #[cfg(target_os = "linux")]
        {
            write_fd(self.fd.as_raw_fd(), packet)
        }

        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            unsupported_write_packet(packet)
        }
    }

    /// Set the fd to non-blocking mode.
    pub fn set_nonblocking(&self) -> io::Result<()> {
        let flags = unsafe { libc::fcntl(self.fd.as_raw_fd(), libc::F_GETFL) };
        if flags < 0 {
            return Err(io::Error::last_os_error());
        }
        let ret =
            unsafe { libc::fcntl(self.fd.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) };
        if ret < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /// Raw fd for use with async I/O (e.g. tokio::io::unix::AsyncFd).
    pub fn raw_fd(&self) -> RawFd {
        self.fd.as_raw_fd()
    }
}

impl Drop for TunDevice {
    fn drop(&mut self) {
        log::info!("TUN: closing device {}", self.name);
        // On macOS, the utun device is automatically destroyed when the fd is closed.
        // On Linux, the tun device is destroyed when the fd is closed (if created with IFF_TUN).
        // fd is closed by OwnedFd drop.
    }
}

// ---------------------------------------------------------------------------
// Platform: macOS (utun)
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
fn create_utun() -> io::Result<(OwnedFd, String)> {
    use std::mem;

    // SYSPROTO_CONTROL = 2, AF_SYSTEM = 32, AF_SYS_CONTROL = 2
    const AF_SYSTEM: i32 = 32;
    const SYSPROTO_CONTROL: i32 = 2;
    const AF_SYS_CONTROL: u16 = 2;
    const UTUN_CONTROL_NAME: &[u8] = b"com.apple.net.utun_control\0";
    const CTLIOCGINFO: libc::c_ulong = 0xc0644e03;

    let fd = unsafe { libc::socket(AF_SYSTEM, libc::SOCK_DGRAM, SYSPROTO_CONTROL) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };

    // Get the control ID for utun.
    #[repr(C)]
    struct CtlInfo {
        ctl_id: u32,
        ctl_name: [u8; 96],
    }

    let mut info: CtlInfo = unsafe { mem::zeroed() };
    info.ctl_name[..UTUN_CONTROL_NAME.len()].copy_from_slice(UTUN_CONTROL_NAME);

    let ret = unsafe { libc::ioctl(fd.as_raw_fd(), CTLIOCGINFO, &mut info) };
    if ret < 0 {
        return Err(io::Error::last_os_error());
    }

    // Connect with sc_unit = 0 to let the kernel pick the next available utun index.
    #[repr(C)]
    struct SockaddrCtl {
        sc_len: u8,
        sc_family: u8,
        ss_sysaddr: u16,
        sc_id: u32,
        sc_unit: u32,
        sc_reserved: [u32; 5],
    }

    let addr = SockaddrCtl {
        sc_len: mem::size_of::<SockaddrCtl>() as u8,
        sc_family: AF_SYSTEM as u8,
        ss_sysaddr: AF_SYS_CONTROL,
        sc_id: info.ctl_id,
        sc_unit: 0, // 0 = auto-assign
        sc_reserved: [0; 5],
    };

    let ret = unsafe {
        libc::connect(
            fd.as_raw_fd(),
            &addr as *const SockaddrCtl as *const libc::sockaddr,
            mem::size_of::<SockaddrCtl>() as libc::socklen_t,
        )
    };
    if ret < 0 {
        return Err(io::Error::last_os_error());
    }

    // Get the actual utun name via getsockopt.
    let mut name_buf = [0u8; 32];
    let mut name_len: libc::socklen_t = name_buf.len() as libc::socklen_t;
    let ret = unsafe {
        libc::getsockopt(
            fd.as_raw_fd(),
            SYSPROTO_CONTROL,
            2, // UTUN_OPT_IFNAME
            name_buf.as_mut_ptr() as *mut libc::c_void,
            &mut name_len,
        )
    };
    if ret < 0 {
        return Err(io::Error::last_os_error());
    }
    let name = String::from_utf8_lossy(&name_buf[..name_len as usize])
        .trim_end_matches('\0')
        .to_string();

    Ok((fd, name))
}

// ---------------------------------------------------------------------------
// Platform: Linux (/dev/net/tun)
// ---------------------------------------------------------------------------

#[cfg(target_os = "linux")]
fn create_tun_linux() -> io::Result<(OwnedFd, String)> {
    use std::mem;

    const IFF_TUN: libc::c_short = 0x0001;
    const IFF_NO_PI: libc::c_short = 0x1000;
    const TUNSETIFF: libc::c_ulong = 0x400454ca;

    let fd = unsafe { libc::open(b"/dev/net/tun\0".as_ptr() as *const _, libc::O_RDWR) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };

    #[repr(C)]
    struct Ifreq {
        ifr_name: [u8; 16],
        ifr_flags: libc::c_short,
        _pad: [u8; 22],
    }

    let mut ifr: Ifreq = unsafe { mem::zeroed() };
    let name = b"cdma-pkt0\0";
    ifr.ifr_name[..name.len()].copy_from_slice(name);
    ifr.ifr_flags = IFF_TUN | IFF_NO_PI;

    let ret = unsafe { libc::ioctl(fd.as_raw_fd(), TUNSETIFF, &mut ifr) };
    if ret < 0 {
        return Err(io::Error::last_os_error());
    }

    let dev_name = String::from_utf8_lossy(&ifr.ifr_name)
        .trim_end_matches('\0')
        .to_string();

    Ok((fd, dev_name))
}

// ---------------------------------------------------------------------------
// Interface configuration (shared)
// ---------------------------------------------------------------------------

fn configure_interface(name: &str, local_ip: Ipv4Addr, peer_ip: Ipv4Addr) -> io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        // ifconfig utunN <local> <peer> up
        let status = Command::new("ifconfig")
            .args([name, &local_ip.to_string(), &peer_ip.to_string(), "up"])
            .status()?;
        if !status.success() {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                format!("ifconfig failed with status {}", status),
            ));
        }
    }

    #[cfg(target_os = "linux")]
    {
        // ip addr add <local> peer <peer> dev <name>
        let status = Command::new("ip")
            .args([
                "addr",
                "add",
                &format!("{}/32", local_ip),
                "peer",
                &format!("{}/32", peer_ip),
                "dev",
                name,
            ])
            .status()?;
        if !status.success() {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                format!("ip addr add failed with status {}", status),
            ));
        }
        // ip link set <name> up
        let status = Command::new("ip")
            .args(["link", "set", name, "up"])
            .status()?;
        if !status.success() {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                format!("ip link set up failed with status {}", status),
            ));
        }
    }

    Ok(())
}

/// Enable NAT masquerade for the peer IP so it can reach the internet.
pub fn enable_nat(peer_ip: Ipv4Addr, _tun_name: &str, nat_interface: &str) -> io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        // Enable IP forwarding
        log_command_status(
            "enable macOS IP forwarding",
            Command::new("sysctl")
                .args(["-w", "net.inet.ip.forwarding=1"])
                .status(),
        );

        // Add pf NAT rule. We write a minimal anchor and load it.
        let rule = format!(
            "nat on {nat_interface} from {peer_ip}/32 to any -> ({nat_interface})\npass from {peer_ip}/32 to any\n",
        );
        // Write to a temp file and load via pfctl.
        let tmp_path = format!("/tmp/cdma-pkt-nat-{}.conf", std::process::id());
        std::fs::write(&tmp_path, &rule)?;

        let status = Command::new("pfctl").args(["-ef", &tmp_path]).status()?;

        if let Err(error) = std::fs::remove_file(&tmp_path) {
            log::warn!("TUN: failed to remove temporary pf NAT file {tmp_path}: {error}");
        }

        if !status.success() {
            log::warn!("TUN: pfctl NAT setup returned non-zero (may need root)");
        }
        log::info!(
            "TUN: NAT enabled for {} via pf on {}",
            peer_ip,
            nat_interface
        );
    }

    #[cfg(target_os = "linux")]
    {
        // Enable IP forwarding
        log_command_status(
            "enable Linux IP forwarding",
            Command::new("sysctl")
                .args(["-w", "net.ipv4.ip_forward=1"])
                .status(),
        );

        // iptables masquerade
        let status = Command::new("iptables")
            .args([
                "-t",
                "nat",
                "-A",
                "POSTROUTING",
                "-s",
                &format!("{}/32", peer_ip),
                "-o",
                nat_interface,
                "-j",
                "MASQUERADE",
            ])
            .status()?;
        if !status.success() {
            log::warn!("TUN: iptables NAT setup failed (may need root)");
        }
        log::info!(
            "TUN: NAT enabled for {} via iptables on {}",
            peer_ip,
            nat_interface
        );
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        log_unsupported_nat(peer_ip, _tun_name, nat_interface);
    }

    Ok(())
}

/// Disable NAT rules for the peer IP.
pub fn disable_nat(peer_ip: Ipv4Addr) -> io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        // Disable pf (simplistic — in production would use anchors).
        log_command_status(
            "disable macOS pf",
            Command::new("pfctl").args(["-d"]).status(),
        );
        log::info!("TUN: NAT disabled for {}", peer_ip);
    }

    #[cfg(target_os = "linux")]
    {
        log_command_status(
            "remove Linux iptables masquerade rule",
            Command::new("iptables")
                .args([
                    "-t",
                    "nat",
                    "-D",
                    "POSTROUTING",
                    "-s",
                    &format!("{}/32", peer_ip),
                    "-j",
                    "MASQUERADE",
                ])
                .status(),
        );
        log::info!("TUN: NAT disabled for {}", peer_ip);
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        log_unsupported_nat_disable(peer_ip);
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Low-level fd helpers
// ---------------------------------------------------------------------------

fn read_fd(fd: RawFd, buf: &mut [u8]) -> io::Result<usize> {
    let n = unsafe { libc::read(fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
    if n < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(n as usize)
    }
}

fn write_fd(fd: RawFd, buf: &[u8]) -> io::Result<usize> {
    let n = unsafe { libc::write(fd, buf.as_ptr() as *const libc::c_void, buf.len()) };
    if n < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(n as usize)
    }
}
