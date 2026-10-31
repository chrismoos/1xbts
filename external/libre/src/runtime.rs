use std::sync::{Mutex, MutexGuard, mpsc};
use std::thread::{self, JoinHandle};

use crate::error::{Error, Result, native_status};

static LIBRE_CONTEXT_LOCK: Mutex<()> = Mutex::new(());

// Matches libre's RE_THREAD_WORKERS. Each cross-thread SIP call queues a wake job on this pool.
const ASYNC_WORKERS: u16 = 4;

#[derive(Debug)]
pub struct Library {
    _guard: MutexGuard<'static, ()>,
}

impl Library {
    pub fn init() -> Result<Self> {
        if !libre_sys::LIBRE_AVAILABLE {
            return Err(Error::NativeUnavailable);
        }

        let guard = LIBRE_CONTEXT_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        // SAFETY: initializes the calling thread's RE context while holding LIBRE_CONTEXT_LOCK.
        let status = unsafe { libre_sys::libre_init() };
        native_status("libre_init", status)?;

        Ok(Self { _guard: guard })
    }
}

impl Drop for Library {
    fn drop(&mut self) {
        // SAFETY: the guard is dropped on the thread that initialized it, before the lock is released.
        unsafe { libre_sys::libre_close() };
    }
}

#[derive(Debug)]
pub struct EventLoop {
    handle: Option<JoinHandle<Result<()>>>,
}

impl EventLoop {
    pub fn spawn() -> Result<Self> {
        if !libre_sys::LIBRE_AVAILABLE {
            return Err(Error::NativeUnavailable);
        }

        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let handle = thread::Builder::new()
            .name("libre-main".to_string())
            .spawn(move || {
                let library = match Library::init() {
                    Ok(library) => library,
                    Err(err) => {
                        let _ = ready_tx.send(Err(err));
                        return Ok(());
                    }
                };

                // Async notifications wake polling after cross-thread timer changes and cancellation.
                // SAFETY: this thread owns the initialized RE context and async worker lifetime.
                let async_status = unsafe { libre_sys::re_thread_async_init(ASYNC_WORKERS) };
                if let Err(error) = native_status("re_thread_async_init", async_status) {
                    let _ = ready_tx.send(Err(error));
                    return Ok(());
                }
                if ready_tx.send(Ok(())).is_err() {
                    // SAFETY: polling has not started and this thread owns the async workers.
                    unsafe { libre_sys::re_thread_async_close() };
                    return Ok(());
                }
                // SAFETY: this thread initialized libre, owns the RE context,
                // and runs the event loop until cancellation.
                let status = unsafe { libre_sys::re_main(None) };
                let result = native_status("re_main", status);
                // SAFETY: polling has returned and this thread owns the async workers.
                unsafe { libre_sys::re_thread_async_close() };
                drop(library);
                result
            })
            .map_err(Error::Spawn)?;

        match ready_rx.recv() {
            Ok(Ok(())) => {}
            Ok(Err(err)) => {
                let _ = handle.join();
                return Err(err);
            }
            Err(_) => return Err(Error::EventLoopPanicked),
        }

        Ok(Self {
            handle: Some(handle),
        })
    }

    pub fn cancel(&self) {
        if libre_sys::LIBRE_AVAILABLE {
            let guard = ThreadGuard::enter();
            // SAFETY: the callback cancels polling on its owning thread before async shutdown.
            let status = unsafe {
                libre_sys::re_thread_async(None, Some(cancel_polling), std::ptr::null_mut())
            };
            drop(guard);
            if status != 0 {
                eprintln!("libre cancellation wake failed: {status}");
            }
        }
    }

    pub fn join(mut self) -> Result<()> {
        self.join_inner()
    }

    fn join_inner(&mut self) -> Result<()> {
        let Some(handle) = self.handle.take() else {
            return Ok(());
        };

        handle.join().map_err(|_| Error::EventLoopPanicked)?
    }
}

impl Drop for EventLoop {
    fn drop(&mut self) {
        if self.handle.is_some() {
            self.cancel();
            let _ = self.join_inner();
        }
    }
}

#[derive(Debug)]
pub struct ThreadGuard {
    active: bool,
}

impl ThreadGuard {
    pub fn enter() -> Self {
        if libre_sys::LIBRE_AVAILABLE {
            // SAFETY: re_thread_enter is the documented lock for non-re threads
            // touching the global re context while re_main is polling.
            unsafe { libre_sys::re_thread_enter() };
            Self { active: true }
        } else {
            Self { active: false }
        }
    }
}

impl Drop for ThreadGuard {
    fn drop(&mut self) {
        if self.active {
            // SAFETY: paired with ThreadGuard::enter on the current thread.
            unsafe { libre_sys::re_thread_leave() };
        }
    }
}

unsafe extern "C" fn cancel_polling(_: std::ffi::c_int, _: *mut std::ffi::c_void) {
    // SAFETY: libre invokes this callback on its active polling thread.
    unsafe { libre_sys::re_cancel() };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlapping_spawn_waits_for_the_running_loop_to_exit() {
        if !libre_sys::LIBRE_AVAILABLE {
            return;
        }
        let first = EventLoop::spawn().unwrap();
        let second = thread::spawn(EventLoop::spawn);
        thread::sleep(std::time::Duration::from_millis(100));
        first.cancel();
        first.join().unwrap();
        let second = second.join().unwrap().unwrap();
        second.cancel();
        second.join().unwrap();
    }

    #[test]
    fn event_loop_can_start_and_stop_repeatedly_without_network_activity() {
        if !libre_sys::LIBRE_AVAILABLE {
            return;
        }
        for _ in 0..5 {
            let event_loop = EventLoop::spawn().unwrap();
            event_loop.cancel();
            event_loop.join().unwrap();
        }
    }
}
