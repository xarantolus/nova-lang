#![no_main]

use std::cell::RefCell;

use libfuzzer_sys::fuzz_target;
use nova::VmContext;

thread_local! {
    static ENGINE: RefCell<VmContext<'static>> = RefCell::new({
        let mut engine = nova::VmContext::new();
        engine.set_operations_limit(10_000);
        engine
    });
}

fuzz_target!(|script: &[u8]| {
    ENGINE.with(|engine| {
        let mut engine = engine.borrow_mut();
        // Reuse the same engine instance across fuzz cases.
        let _ = engine.run(script);
    });
});
