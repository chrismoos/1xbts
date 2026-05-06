use crate::error::{check_status, Error};
use std::ptr;

/// A range of floating-point values, and a step-by amount
#[derive(Clone)]
pub struct Range(uhd_sys::uhd_range_t);

impl Default for Range {
    fn default() -> Self {
        Range(uhd_sys::uhd_range_t {
            start: 0.0,
            stop: 0.0,
            step: 0.0,
        })
    }
}

/// A list of ranges of floating-point values
///
/// The ranges in a meta-range should be monotonic (the start of each range should be greater
/// than or equal to the end of the preceding range). Gaps between ranges are allowed.
///
/// Most MetaRange methods will return errors if called on a non-monotonic range.
pub struct MetaRange(uhd_sys::uhd_meta_range_handle);

impl MetaRange {
    /// Creates an empty meta-range
    pub fn new() -> Result<Self, Error> {
        let mut handle = ptr::null_mut();
        check_status(unsafe { uhd_sys::uhd_meta_range_make(&mut handle) })?;
        Ok(MetaRange(handle))
    }

    /// Returns the overall start of this meta-range
    pub fn start(&self) -> Result<f64, Error> {
        let mut start = 0.0;
        check_status(unsafe { uhd_sys::uhd_meta_range_start(self.0, &mut start) })?;
        Ok(start)
    }

    /// Returns the overall end (stop) of this meta-range
    pub fn stop(&self) -> Result<f64, Error> {
        let mut stop = 0.0;
        check_status(unsafe { uhd_sys::uhd_meta_range_stop(self.0, &mut stop) })?;
        Ok(stop)
    }

    /// Returns the "overall step value" of this meta-range (the minimum of the step values of
    /// each contained range, and the gaps between ranges)
    pub fn step(&self) -> Result<f64, Error> {
        let mut step = 0.0;
        check_status(unsafe { uhd_sys::uhd_meta_range_step(self.0, &mut step) })?;
        Ok(step)
    }

    /// Returns the number of ranges in this meta-range
    pub fn len(&self) -> Result<usize, Error> {
        let mut length = 0usize;
        check_status(unsafe {
            uhd_sys::uhd_meta_range_size(self.0, &mut length as *mut usize as *mut _)
        })?;
        Ok(length)
    }

    /// Returns whether this meta-range contains no ranges.
    pub fn is_empty(&self) -> Result<bool, Error> {
        Ok(self.len()? == 0)
    }

    /// Returns the range at the provided index, if one exists
    pub fn get(&self, index: usize) -> Result<Option<Range>, Error> {
        let mut range = Range::default();
        match check_status(unsafe { uhd_sys::uhd_meta_range_at(self.0, index as _, &mut range.0) })
        {
            Ok(()) => Ok(Some(range)),
            Err(e) => match e {
                // StdExcept usually indicates a std::out_of_range because index >= length
                Error::StdExcept => Ok(None),
                _ => Err(e),
            },
        }
    }
    /// Appends a range to the end of this meta-range
    pub fn push(&mut self, range: Range) -> Result<(), Error> {
        check_status(unsafe { uhd_sys::uhd_meta_range_push_back(self.0, &range.0) })
    }

    /// Returns an iterator over ranges in this meta-range
    pub fn iter(&self) -> Result<Iter<'_>, Error> {
        Ok(Iter {
            range: self,
            next: 0,
            length: self.len()?,
        })
    }

    pub(crate) fn handle(&mut self) -> uhd_sys::uhd_meta_range_handle {
        self.0
    }
}

impl Drop for MetaRange {
    fn drop(&mut self) {
        let _ = unsafe { uhd_sys::uhd_meta_range_free(&mut self.0) };
    }
}

/// An iterator over ranges in a meta-range
pub struct Iter<'m> {
    range: &'m MetaRange,
    /// The index of the next element to yield
    /// Invariant: next <= length
    next: usize,
    length: usize,
}

impl Iterator for Iter<'_> {
    type Item = Result<Range, Error>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.next == self.length {
            None
        } else {
            let item = match self.range.get(self.next) {
                Ok(Some(range)) => Ok(range),
                Ok(None) => return None,
                Err(e) => Err(e),
            };
            self.next += 1;
            Some(item)
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.length - self.next;
        (remaining, Some(remaining))
    }

    fn count(self) -> usize
    where
        Self: Sized,
    {
        self.length - self.next
    }
}

impl ExactSizeIterator for Iter<'_> {}

mod fmt {
    use super::{MetaRange, Range};
    use std::fmt::{Debug, Formatter, Result};

    impl Debug for MetaRange {
        fn fmt(&self, f: &mut Formatter<'_>) -> Result {
            match self.iter() {
                Ok(iter) => f.debug_list().entries(iter.map(|r| r.ok())).finish(),
                Err(e) => write!(f, "<error querying length: {}>", e),
            }
        }
    }

    impl Debug for Range {
        fn fmt(&self, f: &mut Formatter<'_>) -> Result {
            f.debug_struct("Range")
                .field("start", &self.0.start)
                .field("stop", &self.0.stop)
                .field("step", &self.0.step)
                .finish()
        }
    }
}
