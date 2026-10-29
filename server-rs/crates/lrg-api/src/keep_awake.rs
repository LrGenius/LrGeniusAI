//! Keeps the computer from going to sleep while photos are being indexed.
//!
//! An indexing run is a long series of short requests (one per photo, or one
//! per group), and nothing in it counts as user activity, so idle sleep cuts a
//! run left alone into slices. On 2026-10-28 a Mac indexing overnight stopped
//! for ~17 minutes at a time and only moved in the minute or two of each dark
//! wake, where the full load then ended in "Dark Wake Thermal Emergency".
//!
//! While an index batch is in flight, and for [`LINGER`] after the last one,
//! this holds a `PreventUserIdleSystemSleep` power assertion, which is what
//! `caffeinate -i` does: the display may still sleep, and closing the lid still
//! puts the Mac to sleep. The linger is what makes it cover a run. Requests
//! follow each other a few tens of milliseconds apart, and a computer whose
//! idle timer ran out long ago would go to sleep in that gap.
//!
//! macOS only; elsewhere [`KeepAwake::hold`] only counts. Windows' equivalent,
//! `SetThreadExecutionState`, is per thread, which async handlers cannot keep.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How long the computer stays awake after the last batch finished.
pub const LINGER: Duration = Duration::from_secs(120);

/// The name macOS shows for the assertion (`pmset -g assertions`).
const REASON: &str = "LrGeniusAI is indexing photos";

#[derive(Default)]
pub struct KeepAwake {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    in_flight: usize,
    last_done: Option<Instant>,
    /// Whether this process is keeping the computer awake right now.
    awake: bool,
    /// `None` while not awake, and on a platform without power assertions.
    assertion: Option<platform::Assertion>,
    /// Set after the first failure to take the assertion, so a run logs it
    /// once rather than once per photo.
    warned: bool,
}

/// Keeps the computer awake until dropped, and for [`LINGER`] after that.
pub struct Hold {
    owner: Arc<KeepAwake>,
}

impl KeepAwake {
    pub fn hold(self: &Arc<Self>) -> Hold {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.in_flight += 1;
        if !inner.awake {
            inner.awake = true;
            match platform::Assertion::new(REASON) {
                Ok(assertion) => {
                    if assertion.is_some() {
                        log::info!("Keeping the computer awake while photos are indexed");
                    }
                    inner.assertion = assertion;
                }
                // Logged, not reported to the user: they did not ask for this,
                // the photos index the same either way, and there is nothing
                // they could do about it. Their own energy settings still apply.
                Err(e) if !inner.warned => {
                    inner.warned = true;
                    log::warn!("Could not keep the computer awake while indexing: {e}");
                }
                Err(_) => {}
            }
        }
        Hold {
            owner: Arc::clone(self),
        }
    }

    /// Lets the computer sleep again once no batch has run for [`LINGER`].
    /// Called from the maintenance loop, every few seconds.
    pub fn release_if_idle(&self) {
        self.release_if_idle_at(Instant::now());
    }

    fn release_if_idle_at(&self, now: Instant) {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let idle_long_enough = inner
            .last_done
            .is_some_and(|done| now.saturating_duration_since(done) >= LINGER);
        if inner.awake && inner.in_flight == 0 && idle_long_enough {
            inner.awake = false;
            if inner.assertion.take().is_some() {
                log::info!("Indexing has stopped; the computer may sleep again");
            }
        }
    }

    pub fn is_awake(&self) -> bool {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).awake
    }
}

impl Drop for Hold {
    fn drop(&mut self) {
        let mut inner = self.owner.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.in_flight = inner.in_flight.saturating_sub(1);
        inner.last_done = Some(Instant::now());
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use std::ffi::{c_char, c_void, CString};

    type CFStringRef = *const c_void;

    const K_CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;
    const K_IOPM_ASSERTION_LEVEL_ON: u32 = 255;

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFStringCreateWithCString(
            alloc: *const c_void,
            c_str: *const c_char,
            encoding: u32,
        ) -> CFStringRef;
        fn CFRelease(cf: *const c_void);
    }

    #[link(name = "IOKit", kind = "framework")]
    extern "C" {
        fn IOPMAssertionCreateWithName(
            assertion_type: CFStringRef,
            level: u32,
            name: CFStringRef,
            id: *mut u32,
        ) -> i32;
        fn IOPMAssertionRelease(id: u32) -> i32;
    }

    /// A CFString this side owns, released on drop.
    struct CfString(CFStringRef);

    impl CfString {
        fn new(s: &str) -> Result<Self, String> {
            let c = CString::new(s).map_err(|e| e.to_string())?;
            // SAFETY: `c` is a valid NUL-terminated string for the duration of
            // the call; a null allocator means the default one.
            let r = unsafe {
                CFStringCreateWithCString(std::ptr::null(), c.as_ptr(), K_CF_STRING_ENCODING_UTF8)
            };
            if r.is_null() {
                return Err(format!("could not make a CFString of {s:?}"));
            }
            Ok(CfString(r))
        }
    }

    impl Drop for CfString {
        fn drop(&mut self) {
            // SAFETY: created by CFStringCreateWithCString (a +1 reference) and
            // released exactly once.
            unsafe { CFRelease(self.0) }
        }
    }

    /// An IOKit power assertion, released on drop.
    pub(super) struct Assertion(u32);

    impl Assertion {
        pub(super) fn new(reason: &str) -> Result<Option<Self>, String> {
            let kind = CfString::new("PreventUserIdleSystemSleep")?;
            let name = CfString::new(reason)?;
            let mut id = 0u32;
            // SAFETY: both CFStrings are alive for the call and `id` is a valid
            // out-pointer; IOKit retains what it keeps.
            let rc = unsafe {
                IOPMAssertionCreateWithName(kind.0, K_IOPM_ASSERTION_LEVEL_ON, name.0, &mut id)
            };
            if rc != 0 {
                return Err(format!("IOPMAssertionCreateWithName returned {rc:#x}"));
            }
            Ok(Some(Assertion(id)))
        }
    }

    impl Drop for Assertion {
        fn drop(&mut self) {
            // SAFETY: `self.0` came from a successful IOPMAssertionCreateWithName
            // and is released exactly once.
            let rc = unsafe { IOPMAssertionRelease(self.0) };
            if rc != 0 {
                log::warn!("IOPMAssertionRelease returned {rc:#x}");
            }
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    pub(super) struct Assertion;

    impl Assertion {
        pub(super) fn new(_reason: &str) -> Result<Option<Self>, String> {
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stays_awake_through_a_run_and_for_the_linger_after_it() {
        let keep = Arc::new(KeepAwake::default());
        assert!(!keep.is_awake());

        let first = keep.hold();
        let second = keep.hold();
        assert!(keep.is_awake());
        drop(first);
        // One batch still in flight: no amount of time releases it.
        keep.release_if_idle_at(Instant::now() + LINGER * 10);
        assert!(keep.is_awake());

        drop(second);
        let done = Instant::now();
        // The gap between two photos of a run.
        keep.release_if_idle_at(done + Duration::from_millis(50));
        assert!(keep.is_awake());
        keep.release_if_idle_at(done + LINGER + Duration::from_secs(1));
        assert!(!keep.is_awake());

        // A later run takes it again.
        let again = keep.hold();
        assert!(keep.is_awake());
        drop(again);
    }

    #[test]
    fn never_held_means_nothing_to_release() {
        let keep = Arc::new(KeepAwake::default());
        keep.release_if_idle_at(Instant::now() + LINGER * 2);
        assert!(!keep.is_awake());
    }
}
