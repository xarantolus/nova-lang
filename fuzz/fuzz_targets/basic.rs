#![no_main]

use libfuzzer_sys::fuzz_target;
use nova::VmContext;

fuzz_target!(|script: &[u8]| {
    let mut engine: VmContext<'_, '_> = nova::VmContext::new(script);
    engine.set_operations_limit(10_000);
    // Basically just check that we don't panic
    let _ = engine.run();
});
