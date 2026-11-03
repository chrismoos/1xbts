#[cfg(target_os = "linux")]
use std::sync::Once;
use std::sync::atomic::{AtomicU64, Ordering};

use log::{info, warn};

use super::RealtimeSettings;
use num::complex::Complex32;

static DEGRADED_EVENTS: AtomicU64 = AtomicU64::new(0);
#[cfg(target_os = "linux")]
static LINUX_RT_REPORT: Once = Once::new();

pub(crate) fn degraded_events() -> u64 {
    DEGRADED_EVENTS.load(Ordering::Relaxed)
}

fn degraded(label: &str, detail: impl std::fmt::Display) {
    DEGRADED_EVENTS.fetch_add(1, Ordering::Relaxed);
    warn!("{label}: real-time setup degraded: {detail}; continuing without the requested setting");
}

pub(crate) fn apply_tx(settings: &RealtimeSettings) {
    apply_current("bts-tx", settings, settings.tx_priority, settings.tx_cpu);
}

pub(crate) fn apply_rx(settings: &RealtimeSettings) {
    apply_current("bts-rx", settings, settings.rx_priority, settings.rx_cpu);
}

/// The reader is spawned from `bts-rx` and does every radio read. Linux
/// threads inherit the creator's scheduling, macOS threads do not.
pub(crate) fn apply_rx_reader(settings: &RealtimeSettings) {
    apply_current(
        "bts-rx-reader",
        settings,
        settings.rx_priority,
        settings.rx_cpu,
    );
}

fn apply_current(label: &str, settings: &RealtimeSettings, priority: i32, cpu: Option<usize>) {
    if !settings.enabled {
        info!("{label}: real-time scheduling disabled");
        return;
    }

    #[cfg(not(target_os = "linux"))]
    let _ = priority;

    #[cfg(target_os = "linux")]
    apply_linux(label, priority, cpu);

    #[cfg(target_os = "macos")]
    apply_macos(label, cpu);

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = (priority, cpu);
        degraded(label, "real-time scheduling is unsupported on this OS");
    }
}

#[cfg(target_os = "linux")]
fn apply_linux(label: &str, priority: i32, cpu: Option<usize>) {
    LINUX_RT_REPORT.call_once(|| {
        let runtime = std::fs::read_to_string("/proc/sys/kernel/sched_rt_runtime_us")
            .ok()
            .and_then(|value| value.trim().parse::<i64>().ok());
        let period = std::fs::read_to_string("/proc/sys/kernel/sched_rt_period_us")
            .ok()
            .and_then(|value| value.trim().parse::<u64>().ok());
        match (runtime, period) {
            (Some(-1), Some(period)) => {
                info!("linux real-time throttling disabled; period_us={period}");
            }
            (Some(runtime), Some(period)) => {
                info!("linux real-time throttling runtime_us={runtime} period_us={period}");
            }
            _ => warn!("linux real-time throttling settings could not be read"),
        }
    });
    let thread = unsafe { libc::pthread_self() };
    let requested = libc::sched_param {
        sched_priority: priority,
    };
    let ret = unsafe { libc::pthread_setschedparam(thread, libc::SCHED_FIFO, &requested) };
    if ret != 0 {
        let mut limit = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        let limit_text = if unsafe { libc::getrlimit(libc::RLIMIT_RTPRIO, &mut limit) } == 0 {
            format!(" RLIMIT_RTPRIO={}/{}", limit.rlim_cur, limit.rlim_max)
        } else {
            String::new()
        };
        degraded(
            label,
            format!("SCHED_FIFO priority {priority} failed (errno={ret});{limit_text}"),
        );
    } else {
        let mut actual_policy = 0;
        let mut actual = libc::sched_param { sched_priority: 0 };
        let verify =
            unsafe { libc::pthread_getschedparam(thread, &mut actual_policy, &mut actual) };
        if verify != 0 || actual_policy != libc::SCHED_FIFO || actual.sched_priority != priority {
            degraded(
                label,
                format!(
                    "requested SCHED_FIFO/{priority}, observed policy={actual_policy} priority={} verify={verify}",
                    actual.sched_priority
                ),
            );
        } else {
            info!("{label}: effective scheduling SCHED_FIFO priority {priority}");
        }
    }

    if let Some(cpu) = cpu {
        let mut set: libc::cpu_set_t = unsafe { std::mem::zeroed() };
        unsafe {
            libc::CPU_ZERO(&mut set);
            libc::CPU_SET(cpu, &mut set);
        }
        let ret = unsafe {
            libc::pthread_setaffinity_np(thread, std::mem::size_of::<libc::cpu_set_t>(), &set)
        };
        if ret != 0 {
            degraded(
                label,
                format!("CPU affinity cpu={cpu} failed (errno={ret})"),
            );
        } else {
            info!("{label}: pinned to CPU {cpu}");
        }
    }
}

#[cfg(target_os = "macos")]
fn apply_macos(label: &str, cpu: Option<usize>) {
    if cpu.is_some() {
        warn!("{label}: CPU affinity is not available through the macOS pthread API");
    }
    if apply_macos_time_constraint(label) {
        return;
    }
    apply_macos_qos(label);
}

#[cfg(target_os = "macos")]
fn apply_macos_qos(label: &str) {
    // SAFETY: The QoS class and relative priority are valid for the current thread.
    let ret = unsafe {
        libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_USER_INTERACTIVE, 0)
    };
    if ret != 0 {
        degraded(
            label,
            format!("QOS_CLASS_USER_INTERACTIVE failed (ret={ret})"),
        );
        return;
    }
    let mut actual = libc::qos_class_t::QOS_CLASS_UNSPECIFIED;
    let mut relative = 0;
    // SAFETY: Both output pointers refer to live storage for the native QoS query.
    let ret =
        unsafe { libc::pthread_get_qos_class_np(libc::pthread_self(), &mut actual, &mut relative) };
    if ret != 0 {
        degraded(label, format!("could not read effective QoS (errno={ret})"));
        return;
    }
    if !matches!(actual, libc::qos_class_t::QOS_CLASS_USER_INTERACTIVE) {
        degraded(
            label,
            format!("requested USER_INTERACTIVE, observed qos={actual:?}"),
        );
    } else {
        info!("{label}: effective QoS USER_INTERACTIVE relative_priority={relative}");
    }
}

#[cfg(target_os = "macos")]
mod mach {
    #[repr(C)]
    pub(super) struct ThreadTimeConstraintPolicy {
        pub period: u32,
        pub computation: u32,
        pub constraint: u32,
        pub preemptible: i32,
    }
    #[repr(C)]
    pub(super) struct MachTimebaseInfo {
        pub numer: u32,
        pub denom: u32,
    }
    /// `thread_extended_info_data_t` from `<mach/thread_info.h>`.
    #[repr(C)]
    pub(super) struct ThreadExtendedInfo {
        pub pth_user_time: u64,
        pub pth_system_time: u64,
        pub pth_cpu_usage: i32,
        pub pth_policy: i32,
        pub pth_run_state: i32,
        pub pth_flags: i32,
        pub pth_sleep_time: i32,
        pub pth_curpri: i32,
        pub pth_priority: i32,
        pub pth_maxpriority: i32,
        pub pth_name: [std::ffi::c_char; 64],
    }
    unsafe extern "C" {
        pub(super) static mach_task_self_: u32;
        pub(super) fn mach_thread_self() -> u32;
        pub(super) fn thread_policy_set(
            thread: u32,
            flavor: u32,
            policy_info: *const ThreadTimeConstraintPolicy,
            count: u32,
        ) -> i32;
        pub(super) fn thread_policy_get(
            thread: u32,
            flavor: u32,
            policy_info: *mut i32,
            count: *mut u32,
            get_default: *mut i32,
        ) -> i32;
        pub(super) fn thread_info(thread: u32, flavor: u32, info: *mut i32, count: *mut u32)
        -> i32;
        pub(super) fn mach_timebase_info(info: *mut MachTimebaseInfo) -> i32;
        pub(super) fn task_threads(task: u32, list: *mut *mut u32, count: *mut u32) -> i32;
        pub(super) fn mach_port_deallocate(task: u32, name: u32) -> i32;
        pub(super) fn vm_deallocate(task: u32, address: usize, size: usize) -> i32;
    }
    pub(super) const THREAD_TIME_CONSTRAINT_POLICY: u32 = 2;
    pub(super) const THREAD_TIME_CONSTRAINT_POLICY_COUNT: u32 = 4;
    pub(super) const THREAD_EXTENDED_INFO: u32 = 5;
    pub(super) const THREAD_EXTENDED_INFO_COUNT: u32 =
        (std::mem::size_of::<ThreadExtendedInfo>() / std::mem::size_of::<i32>()) as u32;
}

#[cfg(target_os = "macos")]
fn apply_macos_time_constraint(label: &str) -> bool {
    let thread = unsafe { mach::mach_thread_self() };
    set_macos_time_constraint(label, thread)
}

#[cfg(target_os = "macos")]
fn set_macos_time_constraint(label: &str, thread: u32) -> bool {
    use mach::*;
    let mut timebase = MachTimebaseInfo { numer: 0, denom: 0 };
    if unsafe { mach_timebase_info(&mut timebase) } != 0 || timebase.numer == 0 {
        degraded(label, "mach_timebase_info failed");
        return false;
    }
    let ns_to_abs = |ns: u64| -> u32 {
        ((ns as u128 * timebase.denom as u128) / timebase.numer as u128) as u32
    };
    let policy = ThreadTimeConstraintPolicy {
        period: ns_to_abs(1_250_000),
        computation: ns_to_abs(800_000),
        constraint: ns_to_abs(1_200_000),
        preemptible: 1,
    };
    let ret = unsafe {
        thread_policy_set(
            thread,
            THREAD_TIME_CONSTRAINT_POLICY,
            &policy,
            THREAD_TIME_CONSTRAINT_POLICY_COUNT,
        )
    };
    if ret != 0 {
        degraded(
            label,
            format!("THREAD_TIME_CONSTRAINT_POLICY failed (ret={ret})"),
        );
        false
    } else {
        let mut actual = ThreadTimeConstraintPolicy {
            period: 0,
            computation: 0,
            constraint: 0,
            preemptible: 0,
        };
        let mut count = THREAD_TIME_CONSTRAINT_POLICY_COUNT;
        let mut get_default = 0;
        let verify = unsafe {
            thread_policy_get(
                thread,
                THREAD_TIME_CONSTRAINT_POLICY,
                (&mut actual as *mut ThreadTimeConstraintPolicy).cast::<i32>(),
                &mut count,
                &mut get_default,
            )
        };
        if verify != 0
            || get_default != 0
            || actual.period == 0
            || actual.computation == 0
            || actual.computation > actual.constraint
        {
            degraded(
                label,
                format!(
                    "time-constraint verification failed ret={verify} default={get_default} actual={}/{}/{}/{}",
                    actual.period, actual.computation, actual.constraint, actual.preemptible,
                ),
            );
            return false;
        }
        info!(
            "{label}: effective time-constraint policy abs_period={} abs_computation={} abs_constraint={} preemptible={}",
            actual.period, actual.computation, actual.constraint, actual.preemptible,
        );
        true
    }
}

/// Send rights to every thread in this task. Port names for one thread are
/// stable across calls, so two lists can be compared by name. Every name in
/// the list holds a reference and must go through `release_thread_ports`.
#[cfg(target_os = "macos")]
fn task_thread_ports() -> Vec<u32> {
    let mut list: *mut u32 = std::ptr::null_mut();
    let mut count: u32 = 0;
    let task = unsafe { mach::mach_task_self_ };
    if unsafe { mach::task_threads(task, &mut list, &mut count) } != 0 || list.is_null() {
        return Vec::new();
    }
    let ports = unsafe { std::slice::from_raw_parts(list, count as usize) }.to_vec();
    unsafe {
        mach::vm_deallocate(
            task,
            list as usize,
            count as usize * std::mem::size_of::<u32>(),
        );
    }
    ports
}

#[cfg(target_os = "macos")]
fn release_thread_ports(ports: &[u32]) {
    let task = unsafe { mach::mach_task_self_ };
    for &port in ports {
        unsafe {
            mach::mach_port_deallocate(task, port);
        }
    }
}

#[cfg(target_os = "macos")]
fn thread_name_and_priority(thread: u32) -> (String, i32) {
    let mut info = std::mem::MaybeUninit::<mach::ThreadExtendedInfo>::zeroed();
    let mut count = mach::THREAD_EXTENDED_INFO_COUNT;
    let ret = unsafe {
        mach::thread_info(
            thread,
            mach::THREAD_EXTENDED_INFO,
            info.as_mut_ptr().cast::<i32>(),
            &mut count,
        )
    };
    if ret != 0 {
        return (String::new(), -1);
    }
    let info = unsafe { info.assume_init() };
    let name = unsafe { std::ffi::CStr::from_ptr(info.pth_name.as_ptr()) }
        .to_string_lossy()
        .into_owned();
    (name, info.pth_curpri)
}

/// A radio driver's USB event and async threads start at timeshare priority,
/// which puts the frame return path for a real-time `bts-tx` behind every
/// other runnable thread on the host. Threads the async runtime names are
/// left alone.
#[cfg(target_os = "macos")]
fn promote_threads_since(label: &str, before: &[u32]) {
    let after = task_thread_ports();
    let mut promoted = 0;
    for &thread in after.iter().filter(|thread| !before.contains(thread)) {
        let (name, priority) = thread_name_and_priority(thread);
        if name.starts_with("tokio") {
            continue;
        }
        if set_macos_time_constraint(label, thread) {
            promoted += 1;
            info!(
                "{label}: promoted driver thread 0x{thread:x} name={name:?} from priority {priority}"
            );
        } else {
            warn!("{label}: could not promote driver thread 0x{thread:x} name={name:?}");
        }
    }
    info!(
        "{label}: driver spawned {} thread(s), promoted {promoted}",
        after.len().saturating_sub(before.len())
    );
    release_thread_ports(&after);
}

/// Raises the current thread while a radio driver spawns its workers, so they
/// inherit the elevated scheduling, and restores it on drop.
///
/// Linux threads inherit `SCHED_FIFO` and CPU affinity from their creator. macOS
/// threads created by `pthread_create` start at `QOS_CLASS_DEFAULT` whatever the
/// creator runs at, so on macOS the guard instead records the task's threads on
/// entry and gives the time-constraint policy to every thread that appeared by
/// the time it drops.
pub(crate) struct DriverPriorityGuard {
    #[cfg(target_os = "linux")]
    saved: Option<LinuxSchedState>,
    #[cfg(target_os = "macos")]
    label: &'static str,
    #[cfg(target_os = "macos")]
    before: Option<Vec<u32>>,
}

#[cfg(target_os = "linux")]
struct LinuxSchedState {
    label: &'static str,
    policy: i32,
    param: libc::sched_param,
    affinity: libc::cpu_set_t,
}

impl DriverPriorityGuard {
    pub(crate) fn enter(label: &'static str, settings: &RealtimeSettings) -> Self {
        #[cfg(target_os = "linux")]
        {
            Self {
                saved: settings
                    .enabled
                    .then(|| enter_linux(label, settings))
                    .flatten(),
            }
        }

        #[cfg(target_os = "macos")]
        {
            Self {
                label,
                before: settings.enabled.then(task_thread_ports),
            }
        }

        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            let _ = (label, settings);
            Self {}
        }
    }
}

#[cfg(target_os = "macos")]
impl Drop for DriverPriorityGuard {
    fn drop(&mut self) {
        let Some(before) = self.before.take() else {
            return;
        };
        promote_threads_since(self.label, &before);
        release_thread_ports(&before);
    }
}

#[cfg(target_os = "linux")]
fn enter_linux(label: &'static str, settings: &RealtimeSettings) -> Option<LinuxSchedState> {
    let mut policy = 0;
    let mut param = libc::sched_param { sched_priority: 0 };
    let mut affinity: libc::cpu_set_t = unsafe { std::mem::zeroed() };
    let read =
        unsafe { libc::pthread_getschedparam(libc::pthread_self(), &mut policy, &mut param) };
    if read != 0 {
        degraded(
            label,
            format!("could not read original scheduling (errno={read})"),
        );
        return None;
    }
    let affinity_read = unsafe {
        libc::pthread_getaffinity_np(
            libc::pthread_self(),
            std::mem::size_of::<libc::cpu_set_t>(),
            &mut affinity,
        )
    };
    if affinity_read != 0 {
        degraded(
            label,
            format!("could not read original CPU affinity (errno={affinity_read})"),
        );
        return None;
    }
    apply_current(
        label,
        settings,
        settings.driver_priority,
        settings.driver_cpu,
    );
    Some(LinuxSchedState {
        label,
        policy,
        param,
        affinity,
    })
}

#[cfg(target_os = "linux")]
impl Drop for DriverPriorityGuard {
    fn drop(&mut self) {
        let Some(saved) = self.saved.take() else {
            return;
        };
        let ret = unsafe {
            libc::pthread_setschedparam(libc::pthread_self(), saved.policy, &saved.param)
        };
        if ret != 0 {
            degraded(
                saved.label,
                format!("failed to restore scheduling (errno={ret})"),
            );
        }
        let affinity_ret = unsafe {
            libc::pthread_setaffinity_np(
                libc::pthread_self(),
                std::mem::size_of::<libc::cpu_set_t>(),
                &saved.affinity,
            )
        };
        if affinity_ret != 0 {
            degraded(
                saved.label,
                format!("failed to restore CPU affinity (errno={affinity_ret})"),
            );
        }
    }
}

pub(crate) fn prefault_complex(buffer: &mut [Complex32]) {
    let elements_per_page = (4096 / std::mem::size_of::<Complex32>()).max(1);
    for sample in buffer.iter_mut().step_by(elements_per_page) {
        unsafe { std::ptr::write_volatile(sample, std::ptr::read_volatile(sample)) };
    }
    if let Some(sample) = buffer.last_mut() {
        unsafe { std::ptr::write_volatile(sample, std::ptr::read_volatile(sample)) };
    }
}
