#![no_std]
#![no_main]

use nova::VmContext;

/// Entry point required by the Cortex-M target.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn Reset() -> ! {
    loop {}
}

/// Called with an arbitrary external input — the compiler cannot see the
/// concrete values, so it must generate code for all reachable paths.
/// If any path through `VmContext::run` can panic, the linker will fail
/// with an undefined reference to `panic_not_allowed`.
#[unsafe(no_mangle)]
pub extern "C" fn nova_run(input: *const u8, len: usize) {
    // SAFETY: caller guarantees the pointer is valid for `len` bytes.
    let input = unsafe { core::slice::from_raw_parts(input, len) };
    let mut vm: VmContext<'_, 8, 8, 8, 2> = VmContext::new();
    let _ = vm.run(input);
}

// In release builds: reference an undefined external symbol so that if any
// code path leading to a panic is not optimized out, the linker will fail.
// See: https://internals.rust-lang.org/t/panic-handler-free-no-std-targets/14697/7
#[cfg(not(debug_assertions))]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    unsafe extern "C" {
        fn panic_not_allowed() -> !;
    }
    unsafe { panic_not_allowed() }
}

// In debug builds: just spin, so the crate still compiles for quick iteration.
#[cfg(debug_assertions)]
#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}
