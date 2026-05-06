use num_complex::{Complex, Complex32, Complex64};
use std::convert::{TryFrom, TryInto};
use std::ffi::{CString, NulError};
use std::marker::PhantomData;

/// Arguments used to create a stream
///
/// The type parameter I defines the item type and host format.
///
/// The default stream arguments use wire format `sc16` and host format `fc32`:
/// ```
/// use uhd::StreamArgs;
/// use num_complex::Complex32;
/// let args = StreamArgs::<Complex32>::new("sc16");
/// ```
///
#[derive(Debug, Clone)]
pub struct StreamArgs<I> {
    host_format: PhantomData<I>,
    wire_format: String,
    args: String,
    channels: Vec<usize>,
}

impl<I> StreamArgs<I> {
    /// Creates stream arguments with the provided wire format, no additional
    /// arguments, and channel 0
    pub fn new<S>(wire_format: S) -> Self
    where
        S: Into<String>,
    {
        StreamArgs {
            wire_format: wire_format.into(),
            ..StreamArgs::default()
        }
    }

    /// Creates a builder, initialized with default arguments, that can be used to configure
    /// the stream arguments
    pub fn builder() -> StreamArgsBuilder<I> {
        StreamArgsBuilder {
            args: StreamArgs::default(),
        }
    }
}

impl<I> Default for StreamArgs<I> {
    /// Creates stream arguments with wire format `sc16`, host format determined by the type `I`,
    /// and default arguments and channels
    fn default() -> Self {
        StreamArgs {
            host_format: PhantomData,
            wire_format: "sc16".to_string(),
            args: "".to_string(),
            // Empty list = just channel 0
            channels: vec![],
        }
    }
}

pub struct StreamArgsBuilder<I> {
    args: StreamArgs<I>,
}

impl<I> StreamArgsBuilder<I> {
    /// Sets the wire data format
    pub fn wire_format(self, wire_format: String) -> Self {
        StreamArgsBuilder {
            args: StreamArgs {
                wire_format,
                ..self.args
            },
        }
    }

    /// Sets additional arguments for the stream
    pub fn args(self, args: String) -> Self {
        StreamArgsBuilder {
            args: StreamArgs { args, ..self.args },
        }
    }

    /// Sets the indexes of channels to stream
    pub fn channels(self, channels: Vec<usize>) -> Self {
        StreamArgsBuilder {
            args: StreamArgs {
                channels,
                ..self.args
            },
        }
    }

    /// Builds a StreamArgs with the configured options
    pub fn build(self) -> StreamArgs<I> {
        self.args
    }
}

/// C-compatible version of StreamArgs
pub(crate) struct StreamArgsC<'args> {
    pub host_format: CString,
    pub wire_format: CString,
    pub args: CString,
    pub channels: &'args [usize],
}

impl<'args, I> TryFrom<&'args StreamArgs<I>> for StreamArgsC<'args>
where
    I: Item,
{
    type Error = NulError;

    fn try_from(args: &'args StreamArgs<I>) -> Result<Self, Self::Error> {
        Ok(StreamArgsC {
            host_format: CString::new(I::FORMAT)?,
            wire_format: CString::new(&*args.wire_format)?,
            args: CString::new(&*args.args)?,
            channels: &args.channels,
        })
    }
}

/// A stream item
pub trait Item {
    /// The format name (examples: `fc32` for Complex<f32>, `sc16` for Complex<i16>)
    const FORMAT: &'static str;
}

impl Item for Complex64 {
    const FORMAT: &'static str = "fc64";
}
impl Item for Complex32 {
    const FORMAT: &'static str = "fc32";
}
impl Item for Complex<i16> {
    const FORMAT: &'static str = "sc16";
}
impl Item for Complex<i8> {
    const FORMAT: &'static str = "sc8";
}

/// A stream command that can be sent to a USRP to control streaming
#[derive(Debug, Clone)]
pub struct StreamCommand {
    pub time: StreamTime,
    pub command_type: StreamCommandType,
}

#[derive(Debug, Clone)]
pub enum StreamCommandType {
    /// UHD_STREAM_MODE_START_CONTINUOUS
    StartContinuous,

    /// UHD_STREAM_MODE_STOP_CONTINUOUS
    StopContinuous,

    /// UHD_STREAM_MODE_NUM_SAMPS_AND_DONE
    CountAndDone(u64),

    /// UHD_STREAM_MODE_NUM_SAMPS_AND_MORE
    CountAndMore(u64),
}

/// When the USRP should begin streaming
#[derive(Debug, Clone)]
pub enum StreamTime {
    Now,
    Later(std::time::Duration),
}

impl StreamCommand {
    /// Converts this command into a C `uhd_stream_cmd_t`
    ///
    /// # Panics
    ///
    /// This function panics if this command is `Later` and contains a time with
    /// a seconds field that is too large for a time_t.
    pub(crate) fn as_c_command(&self) -> uhd_sys::uhd_stream_cmd_t {
        let mut c_cmd = uhd_sys::uhd_stream_cmd_t {
            stream_mode: uhd_sys::uhd_stream_mode_t::UHD_STREAM_MODE_START_CONTINUOUS,
            num_samps: 0,
            stream_now: false,
            time_spec_full_secs: 0,
            time_spec_frac_secs: 0.0,
        };

        match &self.time {
            StreamTime::Now => c_cmd.stream_now = true,
            StreamTime::Later(dur) => {
                c_cmd.time_spec_full_secs = dur.as_secs() as i64;
                c_cmd.time_spec_frac_secs = dur.subsec_millis() as f64 / 1000.0
            }
        }

        // In some versions of UHD, num_samps is a size_t. In other versions, it's a uint64_t.
        // The Rust code always uses u64, and converts here.

        match self.command_type {
            StreamCommandType::StartContinuous => {
                c_cmd.stream_mode = uhd_sys::uhd_stream_mode_t::UHD_STREAM_MODE_START_CONTINUOUS;
            }
            StreamCommandType::StopContinuous => {
                c_cmd.stream_mode = uhd_sys::uhd_stream_mode_t::UHD_STREAM_MODE_STOP_CONTINUOUS
            }
            StreamCommandType::CountAndDone(samples) => {
                c_cmd.stream_mode = uhd_sys::uhd_stream_mode_t::UHD_STREAM_MODE_NUM_SAMPS_AND_DONE;
                c_cmd.num_samps = samples
                    .try_into()
                    .expect("Number of samples too large for size_t");
            }
            StreamCommandType::CountAndMore(samples) => {
                c_cmd.stream_mode = uhd_sys::uhd_stream_mode_t::UHD_STREAM_MODE_NUM_SAMPS_AND_MORE;
                c_cmd.num_samps = samples
                    .try_into()
                    .expect("Number of samples too large for size_t");
            }
        };
        c_cmd
    }
}
