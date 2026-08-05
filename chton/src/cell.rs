// ── chton cell: platform-adaptive interior mutability ──────────────────
//
// Cell2 provides Mutex on native/WASIX and RefCell on
// wasm32-unknown-unknown, with an identical public API. It is the
// interior-mutability primitive for the store surface.

// On native/WASIX (where std is available): std::sync::Mutex
// On wasm32-unknown-unknown:                   std::cell::RefCell

#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
pub type RefMut<'a, T> = std::sync::MutexGuard<'a, T>;

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
pub type RefMut<'a, T> = std::cell::RefMut<'a, T>;

#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
pub type Ref<'a, T> = std::sync::MutexGuard<'a, T>;

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
pub type Ref<'a, T> = std::cell::Ref<'a, T>;

/// Platform-adaptive cell: Mutex on native/WASIX, RefCell on wasm32-unknown-unknown.
#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
pub struct Cell2<T>(std::sync::Mutex<T>);

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
pub struct Cell2<T>(std::cell::RefCell<T>);

impl<T> Cell2<T> {
    #[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
    pub fn new(val: T) -> Self {
        Cell2(std::sync::Mutex::new(val))
    }

    #[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
    pub fn borrow(&self) -> std::sync::MutexGuard<'_, T> {
        self.0.lock().unwrap()
    }

    #[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
    pub fn borrow_mut(&self) -> std::sync::MutexGuard<'_, T> {
        self.0.lock().unwrap()
    }
}

impl<T> Cell2<T> {
    #[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
    pub fn new(val: T) -> Self {
        Cell2(std::cell::RefCell::new(val))
    }

    #[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
    pub fn borrow(&self) -> std::cell::Ref<'_, T> {
        self.0.borrow()
    }

    #[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
    pub fn borrow_mut(&self) -> std::cell::RefMut<'_, T> {
        self.0.borrow_mut()
    }
}
