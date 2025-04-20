use std::cell::RefCell;

#[allow(unused_imports)]
use crate as dylib_hook;

// Thread-local flag to track internal calls
thread_local! {
    static IN_HOOK: RefCell<bool> = RefCell::new(false);
}

pub fn with_hook_protection<F, G, R>(f: F, f2: G) -> R
where
    F: FnOnce() -> R,
    G: FnOnce() -> R,
{
    IN_HOOK.with(|flag| {
        if *flag.borrow() {
            // If already in a hook, bypass and execute the real function
            return f2();
        }
        *flag.borrow_mut() = true; 
        let result = f(); 
        *flag.borrow_mut() = false;
        result
    })
}

pub fn bypass_hooks<F, R>(f: F) -> R
where
    F: FnOnce() -> R,
{
    IN_HOOK.with(|flag| {
        let was_in_hook = *flag.borrow();
        *flag.borrow_mut() = true;
        let result = f();
        *flag.borrow_mut() = was_in_hook;
        result
    })
}

pub fn disable_hooks() {
    IN_HOOK.with(|flag| *flag.borrow_mut() = true);
}

pub fn enable_hooks() {
    IN_HOOK.with(|flag| *flag.borrow_mut() = false);
}


#[macro_export]
macro_rules! create_hooks {
    ($($orig_fn:ident ($($param:ident: $ptype:ty),*) -> $ret:ty),*) => {
        $(
            create_hook!($orig_fn($($param: $ptype),*) -> $ret);
        )*
    };
}

#[macro_export]
macro_rules! create_hook {
    ($orig_fn:ident ($($param:ident: $ptype:ty),*) -> $ret:ty) => {
        #[allow(dead_code)]
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $orig_fn($($param: $ptype),*) -> $ret {
            dylib_hook::with_hook_protection(
                || {
                    $orig_fn::Chain::new().call($($param),*)
                },
                || {
                    $orig_fn::chain_orig($($param),*, &mut $orig_fn::Chain::new())
                }
            )
        }

        #[allow(dead_code)]
        pub mod $orig_fn {
            use super::*;
            use std::sync::{Mutex, atomic::AtomicPtr};
            

            pub static HOOKS: Mutex<Vec<HookFn>> = Mutex::new(vec![]);
            #[derive(Clone)]
            pub struct HookFn {
                pub f: fn($($ptype),*, &mut Chain) -> $ret,
            }

            pub struct Chain {
                index: usize,
            }
            impl Chain {
                pub fn new() -> Self {
                    Chain { index: 0 }
                }
                pub fn call(&mut self, $($param: $ptype),*) -> $ret {
                    let hook = {
                        let hooks = HOOKS.lock().unwrap();
                        hooks.get(self.index).cloned()
                    };
                    match hook {
                        Some(hook) => {
                            self.index += 1;
                            let result = (hook.f)($($param),*, self);
                            result
                        }
                        None => {
                            chain_orig($($param),*, self)
                        }
                    }
                }
            }
            pub fn add_hook(hook: fn($($ptype),*, &mut Chain) -> $ret) {
                let mut hooks = HOOKS.lock().unwrap();
                hooks.push(HookFn { f: hook });
            }

            pub fn chain_orig($($param: $ptype),*, _: &mut Chain) -> $ret {
                call_orig($($param),*)
            }

            pub fn call_orig($($param: $ptype),*) -> $ret {
                use std::sync::LazyLock;

                static REAL: LazyLock<AtomicPtr<libc::c_void>> = LazyLock::new(|| {
                    AtomicPtr::new( unsafe {
                            libc::dlsym(
                                libc::RTLD_NEXT,
                                concat!(stringify!($orig_fn), "\0").as_ptr() as *const c_char,
                            )
                        }
                    )
                });

                unsafe {
                    (::std::mem::transmute::<*const libc::c_void, unsafe extern "C" fn ( $($param: $ptype),* ) -> $ret>(
                        REAL.load(std::sync::atomic::Ordering::SeqCst)
                    ))($($param),*)
                }
            }
        }
    };
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::{ffi::{c_char, c_int}, cell::RefCell};

    #[test]
    fn single_hook() {
        create_hook!(open(cpath: *const c_char, oflag: c_int) -> c_int);
        thread_local! {
            static CALLED: RefCell<bool> = RefCell::new(false);
        }

        fn hook_fn(cpath: *const c_char, oflag: c_int, chain: &mut open::Chain) -> c_int {
            CALLED.with(|called| *called.borrow_mut() = true);
            let ret = chain.call(cpath, oflag);
            ret
        }
        open::add_hook(hook_fn);

        let path = std::ffi::CString::new("/etc/passwd").unwrap();
        let fd = unsafe { open(path.as_ptr(), 0) };
        assert!(fd >= 0); // Assuming the file descriptor is valid
        assert!(CALLED.with(|called| *called.borrow()));
    }

    #[test]
    fn multiple_hooks() {
        create_hook!(fopen(cpath: *const c_char, mode: *const c_char) -> *mut libc::FILE);
        thread_local! {
            static HOOK1_CALLED: RefCell<bool> = RefCell::new(false);
            static HOOK2_CALLED: RefCell<bool> = RefCell::new(false);
        }

        fn hook1(cpath: *const c_char, mode: *const c_char, chain: &mut fopen::Chain) -> *mut libc::FILE {
            HOOK1_CALLED.with(|called| *called.borrow_mut() = true);
            chain.call(cpath, mode)
        }

        fn hook2(cpath: *const c_char, mode: *const c_char, chain: &mut fopen::Chain) -> *mut libc::FILE {
            HOOK2_CALLED.with(|called| *called.borrow_mut() = true);
            chain.call(cpath, mode)
        }

        fopen::add_hook(hook1);
        fopen::add_hook(hook2);

        let path = std::ffi::CString::new("/etc/passwd").unwrap();
        let mode = std::ffi::CString::new("r").unwrap();
        let file = unsafe { fopen(path.as_ptr(), mode.as_ptr()) };
        assert!(!file.is_null()); // Assuming the file pointer is valid

        assert!(HOOK1_CALLED.with(|called| *called.borrow()));
        assert!(HOOK2_CALLED.with(|called| *called.borrow()));
    }

    #[test]
    fn early_return_hook() {
        create_hook!(openat(dirfd: c_int, cpath: *const c_char, oflag: c_int) -> c_int);
        thread_local! {
            static HOOK1_CALLED: RefCell<bool> = RefCell::new(false);
            static HOOK2_CALLED: RefCell<bool> = RefCell::new(false);
        }

        fn hook1(_dirfd: c_int, _cpath: *const c_char, _oflag: c_int, _chain: &mut openat::Chain) -> c_int {
            HOOK1_CALLED.with(|called| *called.borrow_mut() = true);
            0 // Early return, bypassing the chain
        }

        fn hook2(dirfd: c_int, cpath: *const c_char, oflag: c_int, chain: &mut openat::Chain) -> c_int {
            HOOK2_CALLED.with(|called| *called.borrow_mut() = true);
            chain.call(dirfd, cpath, oflag)
        }

        openat::add_hook(hook1);
        openat::add_hook(hook2);

        let path = std::ffi::CString::new("/etc/passwd").unwrap();
        let fd = unsafe { openat(libc::AT_FDCWD, path.as_ptr(), 0) };
        assert_eq!(fd, 0); // Ensure the early return value is respected

        assert!(HOOK1_CALLED.with(|called| *called.borrow()));
        assert!(!HOOK2_CALLED.with(|called| *called.borrow())); // Ensure hook2 was not called
    }

    #[test]
    fn hook_protection() {
        create_hook!(open64(cpath: *const c_char, oflag: c_int) -> c_int);
        thread_local! {
            static HOOK_CALLED: RefCell<bool> = RefCell::new(false);
        }

        fn hook_fn(_cpath: *const c_char, _oflag: c_int, _chain: &mut open64::Chain) -> c_int {
            HOOK_CALLED.with(|called| *called.borrow_mut() = true);
            -1
        }

        open64::add_hook(hook_fn);

        // Simulate an internal call using with_hook_protection
        let result = with_hook_protection(
            || {
                // Internal call
                let path = std::ffi::CString::new("/etc/passwd").unwrap();
                unsafe { open64(path.as_ptr(), 0) }
            },
            || { -1 },
        );

        assert_ne!(result, -1); 
        assert!(!HOOK_CALLED.with(|called| *called.borrow()));
    }

    #[test]
    fn orig_bypasses_hooks() {
        create_hook!(fopen64(cpath: *const c_char, mode: *const c_char) -> *mut libc::FILE);
        thread_local! {
            static HOOK_CALLED: RefCell<bool> = RefCell::new(false);
        }

        fn hook_fn(cpath: *const c_char, mode: *const c_char, chain: &mut fopen64::Chain) -> *mut libc::FILE {
            HOOK_CALLED.with(|called| *called.borrow_mut() = true);
            chain.call(cpath, mode)
        }

        fopen64::add_hook(hook_fn);

        // Call the original function directly, bypassing hooks
        let path = std::ffi::CString::new("/etc/passwd").unwrap();
        let mode = std::ffi::CString::new("r").unwrap();
        let file = fopen64::call_orig(path.as_ptr(), mode.as_ptr());
        assert!(!file.is_null()); // Assuming the file pointer is valid

        // Ensure the hook was not called
        assert!(!HOOK_CALLED.with(|called| *called.borrow()));
    }
}
