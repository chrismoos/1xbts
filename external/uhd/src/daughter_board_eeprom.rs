use crate::utils::copy_string;
use crate::{check_status, Error};
use std::ffi::CString;
use std::os::raw::c_int;
use std::ptr;

/// Information from the EEPROM of a daughter board
pub struct DaughterBoardEeprom(uhd_sys::uhd_dboard_eeprom_handle);

impl DaughterBoardEeprom {
    pub fn id(&self) -> Result<String, Error> {
        copy_string(|buffer, length| unsafe {
            uhd_sys::uhd_dboard_eeprom_get_id(self.0, buffer, length as _)
        })
    }

    pub fn set_id(&mut self, id: &str) -> Result<(), Error> {
        let id_c = CString::new(id)?;
        check_status(unsafe { uhd_sys::uhd_dboard_eeprom_set_id(self.0, id_c.as_ptr()) })
    }

    pub fn serial(&self) -> Result<String, Error> {
        copy_string(|buffer, length| unsafe {
            uhd_sys::uhd_dboard_eeprom_get_serial(self.0, buffer, length as _)
        })
    }

    pub fn set_serial(&mut self, serial: &str) -> Result<(), Error> {
        let serial_c = CString::new(serial)?;
        check_status(unsafe { uhd_sys::uhd_dboard_eeprom_set_serial(self.0, serial_c.as_ptr()) })
    }

    pub fn revision(&self) -> Result<c_int, Error> {
        let mut revision = 0;
        check_status(unsafe { uhd_sys::uhd_dboard_eeprom_get_revision(self.0, &mut revision) })?;
        Ok(revision)
    }

    pub fn set_revision(&mut self, revision: c_int) -> Result<(), Error> {
        check_status(unsafe { uhd_sys::uhd_dboard_eeprom_set_revision(self.0, revision) })
    }

    pub(crate) fn handle(&mut self) -> uhd_sys::uhd_dboard_eeprom_handle {
        self.0
    }
}

unsafe impl Send for DaughterBoardEeprom {}
unsafe impl Sync for DaughterBoardEeprom {}

impl DaughterBoardEeprom {
    /// Creates a new daughter board EEPROM handle
    pub fn new() -> Result<Self, Error> {
        let mut handle: uhd_sys::uhd_dboard_eeprom_handle = ptr::null_mut();
        check_status(unsafe { uhd_sys::uhd_dboard_eeprom_make(&mut handle) })?;
        Ok(DaughterBoardEeprom(handle))
    }
}

impl Drop for DaughterBoardEeprom {
    fn drop(&mut self) {
        let _ = unsafe { uhd_sys::uhd_dboard_eeprom_free(&mut self.0) };
    }
}

mod fmt {
    use super::DaughterBoardEeprom;
    use std::fmt::{Debug, Formatter, Result};

    impl Debug for DaughterBoardEeprom {
        fn fmt(&self, f: &mut Formatter<'_>) -> Result {
            let revision = self.revision();
            let revision: &dyn Debug = revision
                .as_ref()
                .map(|rev| rev as &dyn Debug)
                .unwrap_or(&"<error>");

            f.debug_struct("DaughterBoardEeprom")
                .field("id", &self.id().as_deref().unwrap_or("<error>"))
                .field("serial", &self.serial().as_deref().unwrap_or("<error>"))
                .field("revision", revision)
                .finish()
        }
    }
}
