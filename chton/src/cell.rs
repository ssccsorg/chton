// ── chton cell: platform-adaptive interior mutability ──────────────────
//
// Cell2 provides a critical-section Mutex<RefCell> on native/WASIX and
// other no_std targets, and a RefCell on wasm32-unknown-unknown, with an
// identical public API. It is the interior-mutability primitive for the
// store surface. The critical-section crate works on single-threaded MCU
// targets (no OS, no std) and on multi-threaded hosts alike, so Cell2
// compiles everywhere the store does.
//
// critical-section 1.2 exposes no MutexGuard: `Mutex::borrow` returns a
// shared `&T`, and `critical_section::with` cannot return a borrow out of
// its closure (the closure parameter is higher-ranked, so the guard cannot
// outlive it). The native guards therefore enter the critical section
// directly with `acquire` and hold it for the lifetime of the guard,
// releasing it on drop. The RefCell inside the Mutex adds the same-thread
// reentrancy check: a second borrow in the same thread panics, mirroring
// the wasm RefCell path. Cross-thread access blocks on the critical-section
// implementation until the guard drops, matching the original
// `std::sync::Mutex` semantics.

// Native/WASIX and other no_std targets: critical-section Mutex<RefCell>,
// with guards that hold the critical section for their lifetime.
#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
mod native {
    use core::cell::{Ref as CellRef, RefCell, RefMut as CellRefMut};
    use core::ops::{Deref, DerefMut};

    /// RAII guard that holds the critical section open for its lifetime.
    ///
    /// The guard structs declare this field after the RefCell borrow, so
    /// the borrow flag is cleared (inner field dropped first) before the
    /// critical section is released.
    struct CsGuard(critical_section::RestoreState);

    impl Drop for CsGuard {
        fn drop(&mut self) {
            // Safety: every CsGuard is constructed with the state returned by
            // a matching `acquire` in `Cell2::borrow`/`borrow_mut`, and the
            // RefCell reentrancy check guarantees the acquire/release pairs
            // are balanced and properly nested.
            unsafe { critical_section::release(self.0) }
        }
    }

    /// Shared borrow guard: holds the critical section for its lifetime.
    pub struct Ref<'a, T: ?Sized> {
        inner: CellRef<'a, T>,
        _guard: CsGuard,
    }

    impl<T: ?Sized> Deref for Ref<'_, T> {
        type Target = T;
        fn deref(&self) -> &T {
            &self.inner
        }
    }

    /// Exclusive borrow guard: holds the critical section for its lifetime.
    pub struct RefMut<'a, T: ?Sized> {
        inner: CellRefMut<'a, T>,
        _guard: CsGuard,
    }

    impl<T: ?Sized> Deref for RefMut<'_, T> {
        type Target = T;
        fn deref(&self) -> &T {
            &self.inner
        }
    }

    impl<T: ?Sized> DerefMut for RefMut<'_, T> {
        fn deref_mut(&mut self) -> &mut T {
            &mut self.inner
        }
    }

    /// Platform-adaptive cell: critical-section Mutex<RefCell> on
    /// native/WASIX and other no_std targets.
    pub struct Cell2<T>(critical_section::Mutex<RefCell<T>>);

    impl<T> Cell2<T> {
        pub fn new(val: T) -> Self {
            Cell2(critical_section::Mutex::new(RefCell::new(val)))
        }

        pub fn borrow(&self) -> Ref<'_, T> {
            // Safety: the returned guard releases the acquired critical
            // section on drop, and the RefCell reentrancy check panics on a
            // same-thread second borrow, so every acquire is paired with
            // exactly one release and pairs stay properly nested.
            let state = unsafe { critical_section::acquire() };
            // Safety: the current thread is inside the critical section
            // acquired above; the token is used only while the guard lives.
            let cs = unsafe { critical_section::CriticalSection::new() };
            let inner = self.0.borrow_ref(cs);
            Ref {
                inner,
                _guard: CsGuard(state),
            }
        }

        pub fn borrow_mut(&self) -> RefMut<'_, T> {
            let state = unsafe { critical_section::acquire() };
            let cs = unsafe { critical_section::CriticalSection::new() };
            let inner = self.0.borrow_ref_mut(cs);
            RefMut {
                inner,
                _guard: CsGuard(state),
            }
        }
    }
}

#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
pub use native::{Cell2, Ref, RefMut};

// wasm32-unknown-unknown: plain RefCell, no critical section.
#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
mod wasm {
    use core::cell::RefCell;

    /// Interior borrow guard type returned by [`Cell2::borrow`].
    pub type Ref<'a, T> = core::cell::Ref<'a, T>;

    /// Exclusive borrow guard type returned by [`Cell2::borrow_mut`].
    pub type RefMut<'a, T> = core::cell::RefMut<'a, T>;

    /// Platform-adaptive cell: RefCell on wasm32-unknown-unknown.
    pub struct Cell2<T>(RefCell<T>);

    impl<T> Cell2<T> {
        pub fn new(val: T) -> Self {
            Cell2(RefCell::new(val))
        }

        pub fn borrow(&self) -> Ref<'_, T> {
            self.0.borrow()
        }

        pub fn borrow_mut(&self) -> RefMut<'_, T> {
            self.0.borrow_mut()
        }
    }
}

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
pub use wasm::{Cell2, Ref, RefMut};
