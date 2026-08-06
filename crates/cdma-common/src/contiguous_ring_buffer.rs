//! A growable ring buffer whose logical contents are always one slice.
//!
//! [`std::collections::VecDeque`] is normally the right ring buffer, but its
//! contents can wrap into two slices. DSP and codec APIs often require one
//! contiguous input slice. [`ContiguousRingBuffer`] stores a mirrored copy of
//! each element one logical capacity apart, so a wrapped logical range is
//! still available through [`ContiguousRingBuffer::as_slice`] without moving
//! retained elements.
//!
//! The tradeoff is intentional: allocated storage is twice the reported
//! logical capacity. The type is not intrinsically bounded or synchronized;
//! callers choose when to discard old elements and provide any locking needed
//! for shared access.

/// A growable ring buffer with O(1) front discard and contiguous reads.
pub struct ContiguousRingBuffer<T> {
    storage: Vec<T>,
    start: usize,
    len: usize,
    capacity: usize,
}

impl<T> ContiguousRingBuffer<T> {
    pub const fn new() -> Self {
        Self {
            storage: Vec::new(),
            start: 0,
            len: 0,
            capacity: 0,
        }
    }

    pub const fn len(&self) -> usize {
        self.len
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Logical element capacity. The backing allocation holds twice this
    /// many elements to make wrapped ranges contiguous.
    pub const fn capacity(&self) -> usize {
        self.capacity
    }

    /// Return all retained elements as one contiguous slice.
    pub fn as_slice(&self) -> &[T] {
        &self.storage[self.start..self.start + self.len]
    }

    /// Remove all logical elements while retaining the allocation.
    pub fn clear(&mut self) {
        self.start = 0;
        self.len = 0;
    }

    /// Discard `count` elements from the logical front in O(1).
    ///
    /// # Panics
    ///
    /// Panics when `count` exceeds the current length.
    pub fn discard_front(&mut self, count: usize) {
        assert!(
            count <= self.len,
            "cannot discard more elements than retained"
        );
        if count == self.len {
            self.clear();
            return;
        }
        self.start = (self.start + count) % self.capacity;
        self.len -= count;
    }
}

impl<T: Clone> ContiguousRingBuffer<T> {
    /// Append a slice, growing the mirrored allocation when necessary.
    pub fn extend_from_slice(&mut self, values: &[T]) {
        if values.is_empty() {
            return;
        }
        self.ensure_capacity(self.len + values.len(), &values[0]);

        let tail = (self.start + self.len) % self.capacity;
        let first_len = values.len().min(self.capacity - tail);
        self.storage[tail..tail + first_len].clone_from_slice(&values[..first_len]);
        self.storage[tail + self.capacity..tail + self.capacity + first_len]
            .clone_from_slice(&values[..first_len]);

        let remaining = &values[first_len..];
        if !remaining.is_empty() {
            self.storage[..remaining.len()].clone_from_slice(remaining);
            self.storage[self.capacity..self.capacity + remaining.len()]
                .clone_from_slice(remaining);
        }
        self.len += values.len();
    }

    fn ensure_capacity(&mut self, required: usize, seed: &T) {
        if required <= self.capacity {
            return;
        }

        let retained = self.as_slice().to_vec();
        let new_capacity = required.next_power_of_two();
        let mut storage = vec![seed.clone(); new_capacity * 2];
        storage[..retained.len()].clone_from_slice(&retained);
        storage[new_capacity..new_capacity + retained.len()].clone_from_slice(&retained);
        self.storage = storage;
        self.start = 0;
        self.capacity = new_capacity;
    }
}

impl<T> Default for ContiguousRingBuffer<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::ContiguousRingBuffer;

    #[test]
    fn wrapped_contents_remain_one_slice() {
        let mut buffer = ContiguousRingBuffer::new();
        buffer.extend_from_slice(&[1, 2, 3, 4]);
        assert_eq!(buffer.capacity(), 4);

        buffer.discard_front(3);
        buffer.extend_from_slice(&[5, 6, 7]);

        assert_eq!(buffer.as_slice(), &[4, 5, 6, 7]);
        assert_eq!(buffer.capacity(), 4);
    }

    #[test]
    fn growth_preserves_wrapped_contents() {
        let mut buffer = ContiguousRingBuffer::new();
        buffer.extend_from_slice(&[1, 2, 3, 4]);
        buffer.discard_front(2);
        buffer.extend_from_slice(&[5, 6]);
        buffer.extend_from_slice(&[7, 8]);

        assert_eq!(buffer.as_slice(), &[3, 4, 5, 6, 7, 8]);
        assert_eq!(buffer.capacity(), 8);
    }

    #[test]
    fn clear_reuses_the_allocation() {
        let mut buffer = ContiguousRingBuffer::new();
        buffer.extend_from_slice(&[1, 2, 3]);
        let capacity = buffer.capacity();

        buffer.clear();
        buffer.extend_from_slice(&[4, 5]);

        assert_eq!(buffer.as_slice(), &[4, 5]);
        assert_eq!(buffer.capacity(), capacity);
    }
}
