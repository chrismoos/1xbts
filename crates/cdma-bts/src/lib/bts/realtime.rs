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
    apply_current(
        "bts-tx",
        settings,
        settings.tx_priority,
        settings.tx_cpu,
        true,
    );
}

pub(crate) fn apply_rx(settings: &RealtimeSettings) {
    apply_current(
        "bts-rx",
        settings,
        settings.rx_priority,
        settings.rx_cpu,
        true,
    );
}

fn apply_current(
    label: &str,
    settings: &RealtimeSettings,
    priority: i32,
    cpu: Option<usize>,
    hard_rt: bool,
) {
    if !settings.enabled {
        info!("{label}: real-time scheduling disabled");
        return;
    }

    #[cfg(not(target_os = "linux"))]
    let _ = priority;
    #[cfg(not(target_os = "macos"))]
    let _ = hard_rt;

    #[cfg(target_os = "linux")]
    apply_linux(label, priority, cpu);

    #[cfg(target_os = "macos")]
    apply_macos(label, hard_rt, cpu);

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = (priority, cpu, hard_rt);
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
fn apply_macos(label: &str, hard_rt: bool, cpu: Option<usize>) {
    if cpu.is_some() {
        warn!("{label}: CPU affinity is not available through the macOS pthread API");
    }
    if hard_rt && apply_macos_time_constraint(label) {
        return;
    }
    apply_macos_qos(label);
}

#[cfg(target_os = "macos")]
fn apply_macos_qos(label: &str) {
    unsafe extern "C" {
        fn pthread_set_qos_class_self_np(qos_class: u32, relative_priority: i32) -> i32;
        fn pthread_get_qos_class_np(thread: libc::pthread_t, relative_priority: *mut i32) -> u32;
    }
    const QOS_CLASS_USER_INTERACTIVE: u32 = 0x21;
    let ret = unsafe { pthread_set_qos_class_self_np(QOS_CLASS_USER_INTERACTIVE, 0) };
    if ret != 0 {
        degraded(
            label,
            format!("QOS_CLASS_USER_INTERACTIVE failed (ret={ret})"),
        );
        return;
    }
    let mut relative = 0;
    let actual = unsafe { pthread_get_qos_class_np(libc::pthread_self(), &mut relative) };
    if actual != QOS_CLASS_USER_INTERACTIVE {
        degraded(
            label,
            format!("requested USER_INTERACTIVE, observed qos=0x{actual:x}"),
        );
    } else {
        info!("{label}: effective QoS USER_INTERACTIVE relative_priority={relative}");
    }
}

#[cfg(target_os = "macos")]
fn apply_macos_time_constraint(label: &str) -> bool {
    #[repr(C)]
    struct ThreadTimeConstraintPolicy {
        period: u32,
        computation: u32,
        constraint: u32,
        preemptible: i32,
    }
    #[repr(C)]
    struct MachTimebaseInfo {
        numer: u32,
        denom: u32,
    }
    unsafe extern "C" {
        fn mach_thread_self() -> u32;
        fn thread_policy_set(
            thread: u32,
            flavor: u32,
            policy_info: *const ThreadTimeConstraintPolicy,
            count: u32,
        ) -> i32;
        fn thread_policy_get(
            thread: u32,
            flavor: u32,
            policy_info: *mut i32,
            count: *mut u32,
            get_default: *mut i32,
        ) -> i32;
        fn mach_timebase_info(info: *mut MachTimebaseInfo) -> i32;
    }
    const THREAD_TIME_CONSTRAINT_POLICY: u32 = 2;
    const THREAD_TIME_CONSTRAINT_POLICY_COUNT: u32 = 4;

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
            mach_thread_self(),
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
                mach_thread_self(),
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

pub(crate) struct DriverPriorityGuard {
    #[cfg(target_os = "linux")]
    original_policy: i32,
    #[cfg(target_os = "linux")]
    original_param: libc::sched_param,
    #[cfg(target_os = "linux")]
    original_affinity: libc::cpu_set_t,
    #[cfg(target_os = "macos")]
    original_qos: u32,
    #[cfg(target_os = "macos")]
    original_relative_priority: i32,
    active: bool,
}

impl DriverPriorityGuard {
    pub(crate) fn enter(label: &str, settings: &RealtimeSettings) -> Self {
        if !settings.enabled {
            return Self::inactive();
        }

        #[cfg(target_os = "linux")]
        {
            let mut original_policy = 0;
            let mut original_param = libc::sched_param { sched_priority: 0 };
            let mut original_affinity: libc::cpu_set_t = unsafe { std::mem::zeroed() };
            let read = unsafe {
                libc::pthread_getschedparam(
                    libc::pthread_self(),
                    &mut original_policy,
                    &mut original_param,
                )
            };
            if read != 0 {
                degraded(
                    label,
                    format!("could not read original scheduling (errno={read})"),
                );
                return Self::inactive();
            }
            let affinity_read = unsafe {
                libc::pthread_getaffinity_np(
                    libc::pthread_self(),
                    std::mem::size_of::<libc::cpu_set_t>(),
                    &mut original_affinity,
                )
            };
            if affinity_read != 0 {
                degraded(
                    label,
                    format!("could not read original CPU affinity (errno={affinity_read})"),
                );
                return Self::inactive();
            }
            apply_current(
                label,
                settings,
                settings.driver_priority,
                settings.driver_cpu,
                false,
            );
            Self {
                original_policy,
                original_param,
                original_affinity,
                active: true,
            }
        }

        #[cfg(target_os = "macos")]
        {
            unsafe extern "C" {
                fn pthread_get_qos_class_np(
                    thread: libc::pthread_t,
                    relative_priority: *mut i32,
                ) -> u32;
            }
            let mut original_relative_priority = 0;
            let original_qos = unsafe {
                pthread_get_qos_class_np(libc::pthread_self(), &mut original_relative_priority)
            };
            apply_current(label, settings, settings.driver_priority, None, false);
            Self {
                original_qos,
                original_relative_priority,
                active: true,
            }
        }

        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            let _ = label;
            Self::inactive()
        }
    }

    fn inactive() -> Self {
        Self {
            #[cfg(target_os = "linux")]
            original_policy: 0,
            #[cfg(target_os = "linux")]
            original_param: libc::sched_param { sched_priority: 0 },
            #[cfg(target_os = "linux")]
            original_affinity: unsafe { std::mem::zeroed() },
            #[cfg(target_os = "macos")]
            original_qos: 0,
            #[cfg(target_os = "macos")]
            original_relative_priority: 0,
            active: false,
        }
    }
}

impl Drop for DriverPriorityGuard {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        #[cfg(target_os = "linux")]
        {
            let ret = unsafe {
                libc::pthread_setschedparam(
                    libc::pthread_self(),
                    self.original_policy,
                    &self.original_param,
                )
            };
            if ret != 0 {
                degraded(
                    "radio-driver-init",
                    format!("failed to restore scheduling (errno={ret})"),
                );
            }
            let affinity_ret = unsafe {
                libc::pthread_setaffinity_np(
                    libc::pthread_self(),
                    std::mem::size_of::<libc::cpu_set_t>(),
                    &self.original_affinity,
                )
            };
            if affinity_ret != 0 {
                degraded(
                    "radio-driver-init",
                    format!("failed to restore CPU affinity (errno={affinity_ret})"),
                );
            }
        }
        #[cfg(target_os = "macos")]
        {
            unsafe extern "C" {
                fn pthread_set_qos_class_self_np(qos_class: u32, relative_priority: i32) -> i32;
            }
            const QOS_CLASS_DEFAULT: u32 = 0x15;
            let restore_qos = if self.original_qos == 0 {
                QOS_CLASS_DEFAULT
            } else {
                self.original_qos
            };
            let ret = unsafe {
                pthread_set_qos_class_self_np(restore_qos, self.original_relative_priority)
            };
            if ret != 0 {
                degraded(
                    "radio-driver-init",
                    format!("failed to restore QoS (ret={ret})"),
                );
            }
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
