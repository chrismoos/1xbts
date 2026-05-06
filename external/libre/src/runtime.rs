use std::sync::{Mutex, mpsc};
use std::thread::{self, JoinHandle};

use crate::error::{Error, Result, native_status};

static LIBRE_REFCOUNT: Mutex<usize> = Mutex::new(0);

#[derive(Debug)]
pub struct Library {
    _private: (),
}

impl Library {
    pub fn init() -> Result<Self> {
        if !libre_sys::LIBRE_AVAILABLE {
            return Err(Error::NativeUnavailable);
        }

        let mut refcount = LIBRE_REFCOUNT
            .lock()
            .expect("libre refcount mutex should not be poisoned");

        if *refcount == 0 {
            // SAFETY: libre_init is process-global and serialized by LIBRE_REFCOUNT.
            let status = unsafe { libre_sys::libre_init() };
            native_status("libre_init", status)?;
        }

        *refcount += 1;

        Ok(Self { _private: () })
    }
}

impl Drop for Library {
    fn drop(&mut self) {
        let mut refcount = LIBRE_REFCOUNT
            .lock()
            .expect("libre refcount mutex should not be poisoned");

        if *refcount == 0 {
            return;
        }

        *refcount -= 1;
        if *refcount == 0 {
            // SAFETY: the last Library guard owns process-global shutdown.
            unsafe { libre_sys::libre_close() };
        }
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
                    Ok(library) => {
                        let _ = ready_tx.send(Ok(()));
                        library
                    }
                    Err(err) => {
                        let _ = ready_tx.send(Err(err));
                        return Ok(());
                    }
                };

                // SAFETY: this thread initialized libre, owns the RE context,
                // and runs the event loop until cancellation.
                let status = unsafe { libre_sys::re_main(None) };
                let result = native_status("re_main", status);
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
            // SAFETY: re_cancel is the process-global way to stop re_main.
            unsafe { libre_sys::re_cancel() };
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
