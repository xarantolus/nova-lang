use nova::{InterpreterError, VmContext, engine_module, script_module};

#[engine_module]
struct MathModule {
    pub CONSTANT: u32,
}

#[script_module]
impl MathModule {
    pub fn add(&self, a: i32, b: i32) -> i32 {
        a + b
    }
}

#[engine_module]
struct FancyMathModule {
    pub MAX_INT: i32,
}

#[script_module]
impl FancyMathModule {
    fn set_max(&mut self, max: i32) {
        self.MAX_INT = max;
    }
}

#[test]
fn math_module() {
    use nova::FromEngine;
    let mut math = MathModule { CONSTANT: 41 };
    let mut vm: VmContext<'_> = VmContext::new()
        .add_module(b"math", &mut math)
        .unwrap();
    let result = vm.run(
        br#"
        import math;
        i = math.add(1, math.CONSTANT);
    "#,
    ).unwrap();

    let variable = result.get_var(b"i").unwrap();
    let result: i32 = FromEngine::from_engine(variable).unwrap();
    assert_eq!(result, 42);
}

#[test]
fn math_module_fancy() {
    use nova::FromEngine;
    let mut math = FancyMathModule { MAX_INT: 100 };
    let mut vm: VmContext<'_> = VmContext::new()
        .add_module(b"fancy_math", &mut math)
        .unwrap();
    let result = vm.run(b"import fancy_math; i = fancy_math.MAX_INT;").unwrap();

    let variable = result.get_var(b"i").unwrap();
    let result: i32 = FromEngine::from_engine(variable).unwrap();

    assert_eq!(result, 100);
}

#[test]
fn invalid_function_access() {
    let mut math = MathModule { CONSTANT: 42 };
    let mut vm: VmContext<'_> = VmContext::new()
        .add_module(b"math", &mut math)
        .unwrap();
    assert!(matches!(
        vm.run(b"import math; i = math.subtract(1, 2);"),
        Err(InterpreterError::InvalidModuleFunctionCall {
            func: b"subtract",
            nargs: 2,
        })
    ));
}

#[test]
fn invalid_member_access() {
    let mut math = MathModule { CONSTANT: 42 };
    let mut vm: VmContext<'_> = VmContext::new()
        .add_module(b"math", &mut math)
        .unwrap();
    assert!(matches!(
        vm.run(b"import math; i = math.MAX;"),
        Err(InterpreterError::InvalidModuleMemberAccess { member: b"MAX" })
    ));
}

#[test]
fn dont_set() {
    let mut math = FancyMathModule { MAX_INT: 100 };
    let mut vm: VmContext<'_> = VmContext::new()
        .add_module(b"fancy_math", &mut math)
        .unwrap();
    assert!(matches!(vm.run(b"import fancy_math; fancy_math.MAX_INT = 200;"), Err(_)));
}
