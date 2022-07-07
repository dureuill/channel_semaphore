#![forbid(unsafe_code)]
#![warn(missing_docs)]
#![warn(missing_debug_implementations)]
#![warn(missing_copy_implementations)]

//! Semaphore implementation based on crossbeam-channel.

/// A semaphore that allows to deliver permits representing access to a shared resource.
///
/// As a semaphore is clonable and sendable between threads, it can be used to share access to a resource between
/// threads.
///
/// A typical use case is to limit the number of concurrent connections to a server.
#[derive(Debug, Clone)]
pub struct Semaphore {
    sender: crossbeam_channel::Sender<()>,
    receiver: crossbeam_channel::Receiver<()>,
}

impl Semaphore {
    /// Create a new semaphore with the specified number of permits
    pub fn new(permits: usize) -> Self {
        let (sender, receiver) = crossbeam_channel::bounded(permits);
        Self { sender, receiver }
    }

    /// Currently available amount of permits for this semaphore.
    ///
    /// This is meant primarily for display rather than to ensure that [`self.acquire`] won't block,
    /// as the number of available permits may change between a call to [`self.available_permits`] and the subsequent
    /// call to [`self.acquire`] (TOCTOU vulnerability).
    pub fn available_permits(&self) -> usize {
        // no possible underflow because len < capacity
        self.total_permits() - self.taken_permits()
    }

    /// Total number of permits for this semaphore.
    ///
    /// The value returned by this method will never change during the lifetime of a semaphore.
    pub fn total_permits(&self) -> usize {
        self.receiver.capacity().expect("Unbounded channel")
    }

    /// Number of permits currently taken for this semaphore.
    ///
    /// This is meant primarily for display rather than to ensure that [`self.acquire`] won't block,
    /// as the number of taken permits may change between a call to [`self.taken_permits`] and the subsequent
    /// call to [`self.acquire`] (TOCTOU vulnerability).
    pub fn taken_permits(&self) -> usize {
        self.receiver.len()
    }

    /// Acquires a permit from this semaphore.
    ///
    /// Blocks until a permit is available from this semaphore, then returns it.
    pub fn acquire(&self) -> SemaphorePermit {
        self.sender.send(()).expect("We also hold a sender");
        SemaphorePermit {
            receiver: self.receiver.clone(),
        }
    }

    /// Attempts to acquire a permit from this semaphore, and fails if none is available.
    ///
    /// This never blocks.
    ///
    /// # Error
    ///
    /// - [`TryAcquireError`]: if no permit is available at the time of the attempt.
    pub fn try_acquire(&self) -> Result<SemaphorePermit, TryAcquireError> {
        self.sender
            .try_send(())
            .map_err(|err| match err {
                crossbeam_channel::TrySendError::Full(_) => TryAcquireError,
                crossbeam_channel::TrySendError::Disconnected(_) => {
                    unreachable!("We also hold a receiver")
                }
            })
            .map(|_| SemaphorePermit {
                receiver: self.receiver.clone(),
            })
    }
}

/// A permit delivered by a semaphore.
#[derive(Debug)]
pub struct SemaphorePermit {
    receiver: crossbeam_channel::Receiver<()>,
}

impl SemaphorePermit {
    /// Returns `true` if the other permit was delivered by the same semaphore.
    pub fn same_semaphore(&self, other: &SemaphorePermit) -> bool {
        self.receiver.same_channel(&other.receiver)
    }
}

/// Error that occurs when trying to [`Semaphore::try_acquire`] a permit when none is available.
#[derive(Debug, Clone, Copy)]
pub struct TryAcquireError;

impl Drop for SemaphorePermit {
    fn drop(&mut self) {
        let _ = self
            .receiver
            .recv()
            .expect("Acquire sent a message to create this permit");
    }
}

#[cfg(test)]
mod tests {
    use std::{sync::{atomic::AtomicBool, Arc}, any::Any};

    use super::{Semaphore, TryAcquireError};

    #[test]
    fn basic() {
        let semaphore = Semaphore::new(10);
        assert_eq!(semaphore.available_permits(), 10);
        assert_eq!(semaphore.total_permits(), 10);
        assert_eq!(semaphore.taken_permits(), 0);
        {
            let _permit = semaphore.acquire();
            assert_eq!(semaphore.available_permits(), 9);
            assert_eq!(semaphore.total_permits(), 10);
            assert_eq!(semaphore.taken_permits(), 1);
        }
        assert_eq!(semaphore.available_permits(), 10);
        assert_eq!(semaphore.total_permits(), 10);
        assert_eq!(semaphore.taken_permits(), 0);

        // take but don't bind => immediately released
        let _ = semaphore.acquire();
        assert_eq!(semaphore.taken_permits(), 0);
    }

    #[test]
    fn exhaustion() -> Result<(), TryAcquireError> {
        let semaphore = Semaphore::new(1);

        let _permit = semaphore.try_acquire()?;
        assert!(matches!(semaphore.try_acquire(), Err(TryAcquireError)));
        Ok(())
    }

    #[test]
    fn multithreads() -> Result<(), Box<dyn Any + Send>> {
        let semaphore = Semaphore::new(1);
        let permit = semaphore.acquire();

        let c = Arc::new(AtomicBool::new(false));
        let cloned_semaphore = semaphore.clone();
        let cloned_c = Arc::clone(&c);
        let thread = std::thread::spawn(move || {
            cloned_semaphore.acquire();
            cloned_c.store(true, std::sync::atomic::Ordering::SeqCst);
        });
        std::thread::sleep(std::time::Duration::from_secs(2));
        assert!(!c.load(std::sync::atomic::Ordering::SeqCst));
        std::mem::drop(permit);
        thread.join()?;
        Ok(())
    }
}
