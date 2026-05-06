use std::ffi::CStr;
use std::str::Utf8Error;

/// Information about a receive channel
#[derive(Debug, Clone)]
pub struct ReceiveInfo {
    motherboard_id: String,
    motherboard_name: String,
    motherboard_serial: String,
    daughterboard_id: String,
    daughterboard_serial: String,
    subdev_name: String,
    subdev_spec: String,
    antenna: String,
}

impl ReceiveInfo {
    pub fn motherboard_id(&self) -> &str {
        &self.motherboard_id
    }
    pub fn motherboard_name(&self) -> &str {
        &self.motherboard_name
    }
    pub fn motherboard_serial(&self) -> &str {
        &self.motherboard_serial
    }
    pub fn daughterboard_id(&self) -> &str {
        &self.daughterboard_id
    }
    pub fn daughterboard_serial(&self) -> &str {
        &self.daughterboard_serial
    }
    pub fn subdev_name(&self) -> &str {
        &self.subdev_name
    }
    pub fn subdev_spec(&self) -> &str {
        &self.subdev_spec
    }
    pub fn antenna(&self) -> &str {
        &self.antenna
    }

    pub(crate) unsafe fn from_c(info_c: &uhd_sys::uhd_usrp_rx_info_t) -> Result<Self, Utf8Error> {
        Ok(ReceiveInfo {
            motherboard_id: CStr::from_ptr(info_c.mboard_id).to_str()?.into(),
            motherboard_name: CStr::from_ptr(info_c.mboard_name).to_str()?.into(),
            motherboard_serial: CStr::from_ptr(info_c.mboard_serial).to_str()?.into(),
            daughterboard_id: CStr::from_ptr(info_c.rx_id).to_str()?.into(),
            daughterboard_serial: CStr::from_ptr(info_c.rx_serial).to_str()?.into(),
            subdev_name: CStr::from_ptr(info_c.rx_subdev_name).to_str()?.into(),
            subdev_spec: CStr::from_ptr(info_c.rx_subdev_spec).to_str()?.into(),
            antenna: CStr::from_ptr(info_c.rx_antenna).to_str()?.into(),
        })
    }
}
