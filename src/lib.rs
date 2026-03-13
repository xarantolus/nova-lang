//! Nova is a simple, embedded scripting engine for Rust.
//!
//! Scripts support variables, arithmetic, comparisons, if/else, while loops,
//! user-defined functions, and native modules. All allocations are bounded at
//! compile time via const generic parameters, making the engine suitable for
//! `no_std` environments.
//!
//! # Quick start
//!
//! ```rust
//! use nova::{VmContext, EngineObject};
//!
//! let mut vm: VmContext<'_> = VmContext::new();
//! let result = vm.run(b"x = 6 * 7;").unwrap();
//! assert_eq!(result.get_var(b"x"), Some(&EngineObject::Int(42)));
//! ```
//!
//! The same [`VmContext`] can run multiple independent scripts without
//! reallocation:
//!
//! ```rust
//! use nova::{VmContext, EngineObject};
//!
//! let mut vm: VmContext<'_> = VmContext::new();
//! let r1 = vm.run(b"x = 1;").unwrap();
//! let r2 = vm.run(b"x = 2;").unwrap();
//! assert_eq!(r1.get_var(b"x"), Some(&EngineObject::Int(1)));
//! assert_eq!(r2.get_var(b"x"), Some(&EngineObject::Int(2)));
//! ```
//!
//! # Modules
//!
//! Native Rust functionality can be exposed to scripts via the [`engine_module`]
//! and [`script_module`] proc macros — see their documentation for details.
//!
//! # Limiting execution
//!
//! To guard against runaway scripts, set an operations cap before running:
//!
//! ```rust
//! use nova::{VmContext, InterpreterError};
//!
//! let mut vm: VmContext<'_> = VmContext::new();
//! vm.set_operations_limit(1_000);
//! assert!(matches!(
//!     vm.run(b"i = 0; while true { i = i + 1; }"),
//!     Err(InterpreterError::TooManyOperations),
//! ));
//! ```

#![cfg_attr(not(test), no_std)]
#![cfg_attr(
    not(test),
    deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)
)]
// Suppress dead-code warnings: some items (e.g. Display/Debug impls) are only
// compiled under `detailed_errors` or debug builds, making other items appear
// unused in stripped release builds.
#![allow(dead_code)]

use arrayvec::ArrayVec;

use crate::tokenizer::{Token, Tokenizer};

pub use nova_macros::{engine_module, script_module};

mod tokenizer;

/// Hidden re-export used by the `engine_module` and `script_module` proc macros.
/// Not part of the public API; semver exempted.
#[doc(hidden)]
pub mod __private {
    pub use super::*;
}

/// Trait for implementing function calls on modules.
/// This is separate from `Module` to allow modules that only have member access (e.g. constants) without needing to implement a full call interface.
pub trait ModuleCall {
    fn internal_call<'a>(
        &mut self,
        func: &'a [u8],
        args: &[EngineObject<'a>],
    ) -> Result<EngineObject<'a>, InterpreterError<'a>> {
        Err(InterpreterError::InvalidModuleFunctionCall {
            func,
            nargs: args.len(),
        })
    }
}

/// Trait for implementing member access on modules.
/// This is separate from `Module` to allow modules that only have functions without needing to implement a full member access interface.
pub trait ModuleGet {
    fn internal_get<'a>(&self, member: &'a [u8]) -> Result<EngineObject<'a>, InterpreterError<'a>> {
        Err(InterpreterError::InvalidModuleMemberAccess { member })
    }
}

/// A module represents a library that can be imported and used in scripts, providing bindings to native functionality.
pub trait Module {
    fn call<'a>(
        &mut self,
        func: &'a [u8],
        args: &[EngineObject<'a>],
    ) -> Result<EngineObject<'a>, InterpreterError<'a>>;

    fn get<'a>(&self, member: &'a [u8]) -> Result<EngineObject<'a>, InterpreterError<'a>>;
}

impl<T> Module for T
where
    T: ModuleCall + ModuleGet,
{
    fn call<'a>(
        &mut self,
        func: &'a [u8],
        args: &[EngineObject<'a>],
    ) -> Result<EngineObject<'a>, InterpreterError<'a>> {
        ModuleCall::internal_call(self, func, args)
    }

    fn get<'a>(&self, member: &'a [u8]) -> Result<EngineObject<'a>, InterpreterError<'a>> {
        ModuleGet::internal_get(self, member)
    }
}

/// Trait for converting from engine objects to Rust types.
/// This is used for module function implementations to convert arguments.
pub trait FromEngine<'a>: Sized {
    fn from_engine(obj: &EngineObject<'a>) -> Result<Self, InterpreterError<'a>>;
}

/// Trait for converting from Rust types to engine objects.
/// This is used for module function implementations to convert return values to types that can be used in the script.
pub trait ToEngine<'a> {
    fn to_engine(self) -> Result<EngineObject<'a>, InterpreterError<'a>>;
}

/// Different types of objects that can be manipulated in the engine.
/// This is the main "value" type of the engine, used for variables, function arguments, return values, etc.
#[derive(Clone)]
pub enum EngineObject<'a> {
    Module(usize),
    ModuleMember {
        module: usize,
        name: &'a [u8],
    },
    Function {
        // Position of the function in the script, at the opening brace of arguments. We can jump to it to call it.
        position: usize,
        num_args: usize,
    },
    // A simple integer value.
    Int(i32),
    Bool(bool),
    // A string literal.
    // If it contains escape characters, we have to unescape it before using
    StringLiteral {
        content: &'a [u8],
        has_escape_characters: bool,
    },
    // e.g. in module.member
    MemberAccess {
        name: &'a [u8],
    },
    // An user-defined object. We don't care about its internal structure.
    // Modules should allocate their own memory and return handles representing objects.
    Handle {
        id: u32,
        module: usize,
    },
    Unit,
}

impl<'a> EngineObject<'a> {
    fn is_true(&self) -> Result<bool, InterpreterError<'a>> {
        match self {
            Self::Bool(b) => Ok(*b),
            Self::Int(i) => Ok(*i != 0),
            _ => Err(InterpreterError::InvalidExpressionResult { obj: self.clone() }),
        }
    }
}

impl PartialEq for EngineObject<'_> {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (EngineObject::Bool(a), EngineObject::Bool(b)) => a == b,
            (EngineObject::Int(a), EngineObject::Int(b)) => a == b,
            (EngineObject::Unit, EngineObject::Unit) => true,
            (
                EngineObject::StringLiteral { content: a, .. },
                EngineObject::StringLiteral { content: b, .. },
            ) => a == b,
            _ => false,
        }
    }
}

#[cfg(any(debug_assertions, test, feature = "detailed_errors"))]
impl core::fmt::Display for EngineObject<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            EngineObject::Int(i) => write!(f, "{}", i),
            EngineObject::StringLiteral { content, .. } => {
                write!(
                    f,
                    "\"{}\"",
                    core::str::from_utf8(content).unwrap_or("<invalid utf-8>")
                )
            }
            EngineObject::Module(_) => write!(f, "<module>"),
            EngineObject::ModuleMember { name, .. } => write!(
                f,
                "<module_member:{}>",
                core::str::from_utf8(name).unwrap_or("<invalid utf-8>")
            ),
            EngineObject::Function { position, num_args } => {
                write!(f, "<function({})@{}>", num_args, position)
            }
            EngineObject::Handle { id, .. } => write!(f, "<handle@{}>", id),
            EngineObject::Unit => write!(f, "void"),
            EngineObject::MemberAccess { name } => write!(
                f,
                "<member_access:{}>",
                core::str::from_utf8(name).unwrap_or("<invalid utf-8>")
            ),
            EngineObject::Bool(b) => write!(f, "{}", b),
        }
    }
}

#[cfg(any(debug_assertions, test, feature = "detailed_errors"))]
impl core::fmt::Debug for EngineObject<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        <Self as core::fmt::Display>::fmt(self, f)
    }
}

impl<'a> FromEngine<'a> for i32 {
    fn from_engine(obj: &EngineObject<'a>) -> Result<Self, InterpreterError<'a>> {
        if let EngineObject::Int(i) = obj {
            Ok(*i)
        } else {
            Err(InterpreterError::InvalidTypeConversion {
                from: obj.clone(),
                to: "i32",
            })
        }
    }
}

impl<'a> FromEngine<'a> for &'a str {
    fn from_engine(obj: &EngineObject<'a>) -> Result<Self, InterpreterError<'a>> {
        match obj {
            EngineObject::StringLiteral {
                content,
                has_escape_characters: false,
            } => {
                core::str::from_utf8(content).map_err(|_| InterpreterError::InvalidTypeConversion {
                    from: obj.clone(),
                    to: "&str",
                })
            }
            _ => Err(InterpreterError::InvalidTypeConversion {
                from: obj.clone(),
                to: "&str",
            }),
        }
    }
}

impl<'a> ToEngine<'a> for &'a str {
    fn to_engine(self) -> Result<EngineObject<'a>, InterpreterError<'a>> {
        Ok(EngineObject::StringLiteral {
            content: self.as_bytes(),
            has_escape_characters: false,
        })
    }
}

impl<'a, T: ToEngine<'a>> ToEngine<'a> for Result<T, InterpreterError<'a>> {
    fn to_engine(self) -> Result<EngineObject<'a>, InterpreterError<'a>> {
        self?.to_engine()
    }
}

impl<'a> ToEngine<'a> for () {
    fn to_engine(self) -> Result<EngineObject<'a>, InterpreterError<'a>> {
        Ok(EngineObject::Unit)
    }
}

impl<'a> ToEngine<'a> for i32 {
    fn to_engine(self) -> Result<EngineObject<'a>, InterpreterError<'a>> {
        Ok(EngineObject::Int(self))
    }
}

impl<'a> ToEngine<'a> for u32 {
    fn to_engine(self) -> Result<EngineObject<'a>, InterpreterError<'a>> {
        Ok(EngineObject::Int(self as i32))
    }
}

impl<'a> ToEngine<'a> for bool {
    fn to_engine(self) -> Result<EngineObject<'a>, InterpreterError<'a>> {
        Ok(EngineObject::Bool(self))
    }
}

impl<'a> TryInto<bool> for EngineObject<'a> {
    type Error = InterpreterError<'a>;

    fn try_into(self) -> Result<bool, Self::Error> {
        match self {
            EngineObject::Bool(b) => Ok(b),
            _ => Err(InterpreterError::InvalidTypeConversion {
                from: self,
                to: "bool",
            }),
        }
    }
}

/// Errors that can occur during interpretation.
#[derive(PartialEq)]
pub enum InterpreterError<'a> {
    /// The name provided was not found in the current variable context.
    InvalidName(&'a [u8]),
    /// A module import failed because the module name was not found in the registered modules.
    ModuleNotResolved(&'a [u8]),
    /// An expression resulted in a value that cannot be used, e.g. trying to use a string literal as a condition.
    InvalidExpressionResult {
        obj: EngineObject<'a>,
    },
    /// A nonexistent function call was done on a module, or with the wrong number of arguments.
    InvalidModuleFunctionCall {
        func: &'a [u8],
        nargs: usize,
    },
    /// A nonexistent member was accessed on a module.
    InvalidModuleMemberAccess {
        member: &'a [u8],
    },
    /// Function call on a non-function object.
    InvalidFunctionCall {
        obj: EngineObject<'a>,
    },
    /// Unary operation on a type that does not support it, e.g. negation on a string.
    InvalidUnaryOperation {
        op: Token<'a>,
        token_pos: usize,
        program: &'a [u8],
        obj: EngineObject<'a>,
    },
    /// Attempt to perform a binary operation on incompatible types.
    InvalidBinaryOperation {
        op: Token<'a>,
        token_pos: usize,
        program: &'a [u8],
        left: EngineObject<'a>,
        right: EngineObject<'a>,
    },
    /// Attempt to convert an engine object to a Rust type that it cannot be converted to.
    /// Typically used in [FromEngine::from_engine].
    InvalidTypeConversion {
        from: EngineObject<'a>,
        to: &'static str,
    },
    /// Invalid Token encountered while expecting an operand, e.g. after "5 +"
    InvalidOperandToken {
        token: Token<'a>,
        token_pos: usize,
        program: &'a [u8],
    },
    UnexpectedToken {
        token_pos: usize,
        program: &'a [u8],
        expected: Token<'a>,
        found: Token<'a>,
    },
    /// Attempt to call a function defined with a specific number of arguments with the wrong number of arguments.
    /// e.g. "fn test(a,b) {}; test();"
    FunctionArgsMismatch {
        expected: usize,
        got: usize,
        name: Option<&'a [u8]>,
    },
    /// Continue statement encountered outside of a loop.
    ContinueOutsideLoop,
    /// Break statement encountered outside of a loop.
    BreakOutsideLoop,
    /// Attempt to pop a scope when there are no scopes left.
    ScopeStackEmpty,
    /// Attempt to push a new scope when the scope stack is already at maximum capacity.
    ScopeStackExhausted,
    /// Attempt to pop an expression when there are no expression results left.
    ExpressionStackEmpty,
    /// Attempt to push a new expression when the expression stack is already at maximum capacity.
    ExpressionStackOverflow,
    /// Out of steps when running with a step limit.
    TooManyOperations,
    /// Attempt to declare a new variable when the variable stack is already at maximum capacity.
    VariableStackOverflow,
    /// Attempt to add more modules than specified in the generic parameter.
    TooManyModules,
    /// End of file reached while skipping over a block
    UnexpectedEoF,
    /// An operator was applied to operands that caused an overflow, e.g. "2147483647 + 1"
    OperatorOverflow {
        op: Token<'a>,
        token_pos: usize,
        program: &'a [u8],
    },
    /// Division by zero error, e.g. "5 / 0"
    DivisionByZero,
    // When execution finishes, but not all scopes were closed
    InvalidEndScope,
    // For errors that don't fit any of the above categories, or when more specific information is not available, we can use a custom error message.
    Custom(&'a str),
    /// An internal invariant was violated. This should never happen; if it does, it is a bug in the engine.
    Internal,
}

impl InterpreterError<'_> {
    fn program_info(&self) -> Option<(usize, &[u8])> {
        match self {
            InterpreterError::InvalidUnaryOperation {
                token_pos, program, ..
            }
            | InterpreterError::InvalidBinaryOperation {
                token_pos, program, ..
            }
            | InterpreterError::UnexpectedToken {
                token_pos, program, ..
            }
            | InterpreterError::InvalidOperandToken {
                token_pos, program, ..
            }
            | InterpreterError::OperatorOverflow {
                token_pos, program, ..
            } => Some((*token_pos, *program)),
            _ => None,
        }
    }
}

#[cfg(any(debug_assertions, test, feature = "detailed_errors"))]
impl<'a> core::fmt::Debug for InterpreterError<'a> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            InterpreterError::InvalidName(name) => {
                write!(
                    f,
                    "Invalid name: {}",
                    core::str::from_utf8(name).unwrap_or("<invalid utf-8>")
                )
            }
            InterpreterError::ModuleNotResolved(name) => {
                write!(
                    f,
                    "Module not resolved: {}",
                    core::str::from_utf8(name).unwrap_or("<invalid utf-8>")
                )
            }
            InterpreterError::InvalidExpressionResult { obj } => {
                write!(f, "Invalid expression result: {}", obj)
            }
            InterpreterError::InvalidModuleFunctionCall { func, nargs } => {
                write!(
                    f,
                    "Invalid module function call: {} with {} args",
                    core::str::from_utf8(func).unwrap_or("<invalid utf-8>"),
                    nargs
                )
            }
            InterpreterError::InvalidModuleMemberAccess { member } => {
                write!(
                    f,
                    "Invalid module member access: {}",
                    core::str::from_utf8(member).unwrap_or("<invalid utf-8>")
                )
            }
            InterpreterError::InvalidFunctionCall { obj } => {
                write!(f, "Invalid function call on object: {}", obj)
            }
            InterpreterError::InvalidUnaryOperation { op, obj, .. } => {
                write!(f, "Invalid unary operation: {:?} on object {}", op, obj)
            }
            InterpreterError::InvalidBinaryOperation {
                op, left, right, ..
            } => {
                write!(
                    f,
                    "Invalid binary operation: {:?} on objects {} and {}",
                    op, left, right
                )
            }
            InterpreterError::InvalidTypeConversion { from, to } => {
                write!(f, "Invalid type conversion from {} to {}", from, to)
            }
            InterpreterError::InvalidOperandToken { token, .. } => {
                write!(f, "Invalid operand token: {:?}", token,)
            }
            InterpreterError::UnexpectedToken {
                expected, found, ..
            } => {
                write!(
                    f,
                    "Unexpected token: expected {:?}, found {:?}",
                    expected, found
                )
            }
            InterpreterError::FunctionArgsMismatch {
                expected,
                got,
                name,
            } => {
                write!(
                    f,
                    "Function argument count mismatch for {}: expected {}, got {}",
                    name.map_or("<anonymous>", |n| core::str::from_utf8(n)
                        .unwrap_or("<invalid utf-8>")),
                    expected,
                    got
                )
            }
            InterpreterError::BreakOutsideLoop => write!(f, "Break statement outside of loop"),
            InterpreterError::ContinueOutsideLoop => {
                write!(f, "Continue statement outside of loop")
            }
            InterpreterError::ScopeStackEmpty => {
                write!(f, "Scope stack was empty when popping a scope")
            }
            InterpreterError::ScopeStackExhausted => write!(f, "Scope stack exhausted"),
            InterpreterError::ExpressionStackEmpty => write!(
                f,
                "Expression stack was empty when popping an expression result"
            ),
            InterpreterError::ExpressionStackOverflow => {
                write!(f, "Expression stack overflow (expression tree too deep)")
            }
            InterpreterError::TooManyOperations => {
                write!(f, "Exceeded maximum number of operations")
            }
            InterpreterError::VariableStackOverflow => {
                write!(
                    f,
                    "Variable stack overflow (too many variables in all scope)"
                )
            }
            InterpreterError::TooManyModules => {
                write!(f, "Cannot add more modules: maximum reached")
            }
            InterpreterError::UnexpectedEoF => {
                write!(f, "Unexpected end of file while parsing")
            }
            InterpreterError::OperatorOverflow { op, .. } => {
                write!(f, "Overflow while applying operator {:?}", op)
            }
            InterpreterError::DivisionByZero => {
                write!(f, "Division by zero")
            }
            InterpreterError::InvalidEndScope => {
                write!(
                    f,
                    "Invalid end of scope: not all scopes were closed at end of execution"
                )
            }
            InterpreterError::Custom(msg) => write!(f, "Error: {}", msg),
            InterpreterError::Internal => write!(f, "Internal engine error (this is a bug)"),
        }?;

        if let Some((pos, program)) = self.program_info() {
            // Find left and right until we find newline, so we have reference to full line
            // Calculate start of the current line (needed for column offset)
            let current_line_start = program[..pos]
                .iter()
                .rposition(|&b| b == b'\n')
                .map(|r| r + 1)
                .unwrap_or(0);

            // Calculate start of the snippet (up to 2 lines before)
            let snippet_start = program[..pos]
                .iter()
                .enumerate()
                .rev()
                .filter(|&(_, b)| *b == b'\n')
                .nth(2) // 0=current, 1=prev, 2=prev-prev
                .map(|(index, _)| index + 1)
                .unwrap_or(0);

            let line_end = pos
                + program[pos..]
                    .iter()
                    .position(|&b| b == b'\n')
                    .unwrap_or(program.len() - pos);

            let line = &program[snippet_start..line_end];
            let line_number = program[..pos].iter().filter(|&&b| b == b'\n').count() + 1;
            let col = (pos - current_line_start).saturating_sub(1);

            write!(
                f,
                "\n --> at byte position {} (line {}, column {})\n{}\n{:width$}^",
                pos,
                line_number,
                col + 1,
                core::str::from_utf8(line).unwrap_or("<invalid utf-8>"),
                "",
                width = col
            )?;
        }

        Ok(())
    }
}
/// Enum to track the result of an expression evaluation.
enum EvaluationResult<'a> {
    /// The expression calculated a final value immediately.
    Value(EngineObject<'a>),
    /// The expression encountered a function call and has paused execution.
    /// The VM should continue the main loop to execute the function body.
    Suspended,
}

enum BlockScope<'a> {
    Normal,
    While {
        /// Cursor position immediately after the `while` keyword,
        /// where the condition expression begins.
        condition_start: usize,
    },
    If,
    Else,
    Function {
        // frame pointer for this function call
        return_addr: usize,
        /// The length of the operator stack when this function was called.
        /// We need this to restore the expression parsing context upon return.
        caller_ops_len: usize,
    },
    /// We are evaluating the RHS of an assignment: `name = <expr>`
    Assignment {
        name: &'a [u8],
    },
    /// We are evaluating a return value: `return <expr>`
    Return,
    /// We are evaluating an If condition: `if <expr>`
    IfCondition,
    /// We are evaluating a While condition: `while <expr>`
    WhileCondition {
        condition_start: usize,
    },
    /// We are evaluating a standalone expression statement: `<expr>;`
    ExpressionStatement,
}

/// State saved when we've suspended mid-argument-list for a user-defined function call.
struct FunctionArgState<'a> {
    function_object: EngineObject<'a>,
    args_start: usize,
    outer_ops_len: usize,
}

/// Named variable and value.
struct Variable<'a> {
    name: &'a [u8],
    value: EngineObject<'a>,
}

/// Result of a successful script run, containing the global variables from the executed script.
pub struct RunResult<'a, const STACK_SIZE: usize = 32> {
    variables: ArrayVec<Variable<'a>, STACK_SIZE>,
}

impl<'a, const STACK_SIZE: usize> RunResult<'a, STACK_SIZE> {
    /// Get a global variable by name, returning a reference to its value.
    ///
    /// Returns `None` if no variable with that name exists in the global scope.
    ///
    /// ```rust
    /// use nova::{VmContext, EngineObject};
    ///
    /// let mut vm: VmContext<'_> = VmContext::new();
    /// let result = vm.run(b"answer = 42;").unwrap();
    ///
    /// assert_eq!(result.get_var(b"answer"), Some(&EngineObject::Int(42)));
    /// assert_eq!(result.get_var(b"missing"), None);
    /// ```
    pub fn get_var(&self, name: &[u8]) -> Option<&EngineObject<'a>> {
        self.variables
            .iter()
            .rev()
            .find(|v| v.name == name)
            .map(|v| &v.value)
    }
}

/// Per-run execution state. Created fresh on each call to [`VmContext::run`].
struct Execution<
    'a,
    'vm,
    'm,
    const STACK_SIZE: usize,
    const MAX_SCOPE_DEPTH: usize,
    const MAX_EXPRESSION_DEPTH: usize,
    const MAX_MODULES: usize,
> {
    variables: ArrayVec<Variable<'a>, STACK_SIZE>,
    scope_stack: ArrayVec<BlockScope<'a>, MAX_SCOPE_DEPTH>,
    current_block_scope: ArrayVec<usize, MAX_SCOPE_DEPTH>,
    expression_stack: ArrayVec<EngineObject<'a>, MAX_EXPRESSION_DEPTH>,
    expression_operator_stack: ArrayVec<(Token<'a>, u8), MAX_EXPRESSION_DEPTH>,
    resume_expression: Option<usize>,
    arg_eval_stack: ArrayVec<FunctionArgState<'a>, MAX_SCOPE_DEPTH>,
    tokenizer: Tokenizer<'a>,
    modules: &'vm mut ArrayVec<(&'m [u8], &'m mut dyn Module), MAX_MODULES>,
    operations_limit: usize,
    current_operations: usize,
}

/// The virtual machine context, holding all persistent state between script runs.
///
/// The generic parameters specify various limits for the VM, to allow tuning for different environments and use cases.
/// - `STACK_SIZE`: maximum number of variables that can be in scope at once.
/// - `MAX_SCOPE_DEPTH`: maximum depth of nested blocks (e.g. ifs, loops, functions).
/// - `MAX_EXPRESSION_DEPTH`: maximum depth of nested expressions
/// - `MAX_MODULES`: maximum number of modules that can be registered and imported.
pub struct VmContext<
    'm,
    const STACK_SIZE: usize = 32,
    const MAX_SCOPE_DEPTH: usize = 32,
    const MAX_EXPRESSION_DEPTH: usize = 16,
    const MAX_MODULES: usize = 4,
> {
    modules: ArrayVec<(&'m [u8], &'m mut dyn Module), MAX_MODULES>,
    operations_limit: usize,
}

/// A [`VmContext`] configuration that uses at most **1 KB** of stack per [`run`](VmContext::run).
///
/// Limits: 4 variables, 4 scope levels, 4 expression depth, 2 modules.
pub type VmContextTiny<'m> = VmContext<'m, 4, 4, 4, 2>;

/// A [`VmContext`] configuration that uses at most **2 KB** of stack per [`run`](VmContext::run).
///
/// Limits: 8 variables, 8 scope levels, 8 expression depth, 2 modules.
pub type VmContextSmall<'m> = VmContext<'m, 8, 8, 8, 2>;

/// A [`VmContext`] configuration that uses at most **4 KB** of stack per [`run`](VmContext::run).
///
/// Limits: 16 variables, 16 scope levels, 16 expression depth, 4 modules.
pub type VmContextMedium<'m> = VmContext<'m, 16, 16, 16, 4>;

/// The default [`VmContext`] configuration, using at most **8 KB** of stack per [`run`](VmContext::run).
///
/// Limits: 32 variables, 32 scope levels, 16 expression depth, 4 modules.
pub type VmContextLarge<'m> = VmContext<'m, 32, 32, 16, 4>;

impl<
    'a,
    'vm,
    'm,
    const STACK_SIZE: usize,
    const MAX_SCOPE_DEPTH: usize,
    const MAX_EXPRESSION_DEPTH: usize,
    const MAX_MODULES: usize,
> Execution<'a, 'vm, 'm, STACK_SIZE, MAX_SCOPE_DEPTH, MAX_EXPRESSION_DEPTH, MAX_MODULES>
{
    fn into_result(self) -> RunResult<'a, STACK_SIZE> {
        RunResult {
            variables: self.variables,
        }
    }

    fn run_loop(&mut self) -> Result<(), InterpreterError<'a>> {
        loop {
            match self.step() {
                Err(e) => return Err(e),
                Ok(false) => break,
                Ok(true) => {}
            }
        }
        // When finished, we should only have the global scope left
        if self.scope_stack.len() != 1 || !matches!(self.scope_stack[0], BlockScope::Normal) {
            return Err(InterpreterError::InvalidEndScope);
        }
        Ok(())
    }

    fn check_operations_limit(&mut self) -> Result<(), InterpreterError<'a>> {
        self.current_operations += 1;
        if self.current_operations > self.operations_limit {
            return Err(InterpreterError::TooManyOperations);
        }
        Ok(())
    }

    // Returns: Ok(true) if work was done, Ok(false) if EOF, Err on error
    fn step(&mut self) -> Result<bool, InterpreterError<'a>> {
        self.check_operations_limit()?;

        if let Some(ops_len) = self.resume_expression {
            self.resume_expression = None;
            // Resume expression parsing expecting an operator (since we just got a value back)
            let mut result = self.eval_expr_internal(ops_len, false)?;
            loop {
                match result {
                    EvaluationResult::Suspended => return Ok(true),
                    EvaluationResult::Value(v) => {
                        if !self.arg_eval_stack.is_empty() {
                            result = self.continue_args(v)?;
                        } else {
                            return self.handle_evaluation_result(EvaluationResult::Value(v));
                        }
                    }
                }
            }
        }

        let (first_token, second_token) = self.tokenizer.peek2();

        match (first_token, second_token) {
            (Token::Import, Token::Identifier(module_import_name)) => {
                self.tokenizer.advance();
                self.tokenizer.advance();

                // Peek forward for as, for potentially different name
                let third_token = self.tokenizer.peek();
                let module_var_name = if third_token == Token::As {
                    self.tokenizer.advance(); // consume 'as'
                    match self.tokenizer.advance() {
                        Token::Identifier(alias) => alias,
                        t => {
                            return Err(InterpreterError::UnexpectedToken {
                                expected: Token::Identifier(&[]),
                                token_pos: self.tokenizer.last_token_pos(),
                                program: self.tokenizer.input(),
                                found: t,
                            });
                        }
                    }
                } else {
                    module_import_name
                };

                match self
                    .modules
                    .iter()
                    .position(|(n, _)| *n == module_import_name)
                {
                    Some(idx) => self.set_var(module_var_name, EngineObject::Module(idx))?,
                    None => return Err(InterpreterError::ModuleNotResolved(module_import_name)),
                }
                self.consume_separator()?;
            }
            (Token::Identifier(var_name), Token::Assign) => {
                self.tokenizer.advance();
                self.tokenizer.advance();
                self.enter_scope(BlockScope::Assignment { name: var_name })?;
                let ops_len = self.expression_operator_stack.len();
                let result = self.eval_expr_internal(ops_len, true)?;
                return self.handle_evaluation_result(result);
            }
            (Token::Fn, Token::Identifier(function_name)) => {
                self.consume_token(&Token::Fn)?;
                self.tokenizer.advance();
                self.consume_token(&Token::OpenParen)?;
                let function_pos = self.tokenizer.cursor_pos();

                let mut nargs = 0;
                // Skip function args - we always expect ident, comma
                // Future: maybe allow some kind of type annotations?
                let mut next_ident = true;
                loop {
                    match self.tokenizer.advance() {
                        Token::Identifier(_) if next_ident => {
                            nargs += 1;
                            next_ident = false;
                        }
                        Token::Comma if !next_ident => {
                            next_ident = true;
                        }
                        Token::CloseParen if (!next_ident || nargs == 0) => break,
                        tok => {
                            return Err(InterpreterError::UnexpectedToken {
                                expected: if next_ident {
                                    Token::Identifier(&[])
                                } else {
                                    Token::Comma
                                },
                                token_pos: self.tokenizer.last_token_pos(),
                                program: self.tokenizer.input(),
                                found: tok,
                            });
                        }
                    }
                }

                self.consume_token(&Token::OpenBrace)?;

                self.skip_block(None)?;

                self.consume_separator()?;

                self.set_var(
                    function_name,
                    EngineObject::Function {
                        position: function_pos,
                        num_args: nargs,
                    },
                )?;
            }
            (Token::If, _) => {
                self.tokenizer.advance();
                self.enter_scope(BlockScope::IfCondition)?;
                let ops_len = self.expression_operator_stack.len();
                let result = self.eval_expr_internal(ops_len, true)?;
                return self.handle_evaluation_result(result);
            }
            (Token::Else, _) => {
                // If we hit this, we have an else without a matching if...
                return Err(InterpreterError::UnexpectedToken {
                    expected: Token::If,
                    found: Token::Else,
                    program: self.tokenizer.input(),
                    token_pos: self.tokenizer.last_token_pos(),
                });
            }
            (Token::Return, rest) => {
                self.tokenizer.advance();
                self.enter_scope(BlockScope::Return)?;
                match rest {
                    Token::Separator | Token::Eof => {
                        return self
                            .handle_evaluation_result(EvaluationResult::Value(EngineObject::Unit));
                    }
                    _ => {
                        let ops_len = self.expression_operator_stack.len();
                        let result = self.eval_expr_internal(ops_len, true)?;
                        return self.handle_evaluation_result(result);
                    }
                }
            }
            (Token::While, _) => {
                self.tokenizer.advance();
                let condition_start = self.tokenizer.cursor_pos();
                self.enter_scope(BlockScope::WhileCondition { condition_start })?;
                let ops_len = self.expression_operator_stack.len();
                let result = self.eval_expr_internal(ops_len, true)?;
                return self.handle_evaluation_result(result);
            }
            (Token::Continue, _) => {
                self.tokenizer.advance();
                let (loop_condition_pos, _) = self.pop_loop(true)?;
                return self.evaluate_while_condition(loop_condition_pos);
            }
            (Token::Break, _) => {
                self.tokenizer.advance();
                let (_, scopes_popped) = self.pop_loop(false)?;
                // skip remaining scopes
                self.skip_block(Some(scopes_popped))?;
            }
            (Token::OpenBrace, _) => {}
            (Token::CloseBrace, _) => {
                // End of a block scope ({}, function without return, if, loop)

                // Consume the brace
                self.tokenizer.advance();

                let Some(block) = self.scope_stack.pop() else {
                    return Err(InterpreterError::ScopeStackEmpty);
                };

                // How many variables were created in the block we just popped
                let var_count = self
                    .current_block_scope
                    .pop()
                    .ok_or(InterpreterError::ScopeStackEmpty)?;

                self.variables.truncate(self.variables.len() - var_count);

                match block {
                    BlockScope::Function {
                        return_addr,
                        caller_ops_len,
                    } => {
                        // function ends without return statement -> return unit
                        self.expression_stack
                            .try_push(EngineObject::Unit)
                            .map_err(|_| InterpreterError::ExpressionStackOverflow)?;
                        self.tokenizer.set_cursor(return_addr);
                        self.resume_expression = Some(caller_ops_len);
                    }
                    BlockScope::If => {
                        // If ended, skip over else block if present
                        let next = self.tokenizer.peek();
                        if next == Token::Else {
                            self.tokenizer.advance();
                            self.consume_token(&Token::OpenBrace)?;
                            self.skip_block(None)?;
                        }
                        self.consume_separator()?;
                    }
                    BlockScope::While { condition_start } => {
                        return self.evaluate_while_condition(condition_start);
                    }
                    _ => {
                        self.consume_separator()?;
                    }
                }
            }
            (Token::Separator, Token::Eof) | (Token::Eof, _) => return Ok(false),
            // Anything else is just an expression, e.g. a function call
            _ => {
                self.enter_scope(BlockScope::ExpressionStatement)?;
                let ops_len = self.expression_operator_stack.len();
                let result = self.eval_expr_internal(ops_len, true)?;
                return self.handle_evaluation_result(result);
            }
        }

        Ok(true)
    }

    fn pop_loop(&mut self, is_continue: bool) -> Result<(usize, usize), InterpreterError<'a>> {
        let mut vars_to_remove = 0;
        let mut num_scopes = 0;

        loop {
            let Some(scope) = self.scope_stack.pop() else {
                return Err(if is_continue {
                    InterpreterError::ContinueOutsideLoop
                } else {
                    InterpreterError::BreakOutsideLoop
                });
            };
            vars_to_remove += self
                .current_block_scope
                .pop()
                .ok_or(InterpreterError::ScopeStackEmpty)?;
            num_scopes += 1;

            if let BlockScope::While { condition_start } = scope {
                self.variables
                    .truncate(self.variables.len() - vars_to_remove);

                return Ok((condition_start, num_scopes));
            }
        }
    }

    fn enter_scope(&mut self, scope: BlockScope<'a>) -> Result<(), InterpreterError<'a>> {
        self.scope_stack
            .try_push(scope)
            .map_err(|_| InterpreterError::ScopeStackExhausted)?;
        self.current_block_scope
            .try_push(0)
            .map_err(|_| InterpreterError::ScopeStackExhausted)?;
        Ok(())
    }

    fn evaluate_while_condition(
        &mut self,
        condition_start: usize,
    ) -> Result<bool, InterpreterError<'a>> {
        self.tokenizer.set_cursor(condition_start);
        self.enter_scope(BlockScope::WhileCondition { condition_start })?;
        let ops_len = self.expression_operator_stack.len();
        let result = self.eval_expr_internal(ops_len, true)?;
        self.handle_evaluation_result(result)
    }

    /// Consumes next tokens, ensuring it is the expected one, otherwise returns an error.
    fn consume_token(&mut self, expected: &Token<'a>) -> Result<(), InterpreterError<'a>> {
        let token = self.tokenizer.advance();
        if token == *expected {
            Ok(())
        } else {
            Err(InterpreterError::UnexpectedToken {
                expected: *expected,
                found: token,
                token_pos: self.tokenizer.last_token_pos(),
                program: self.tokenizer.input(),
            })
        }
    }

    /// Consumes separator tokens
    fn consume_separator(&mut self) -> Result<(), InterpreterError<'a>> {
        let token = self.tokenizer.advance();
        if Token::Separator == token || Token::Eof == token {
            Ok(())
        } else {
            Err(InterpreterError::UnexpectedToken {
                expected: Token::Separator,
                token_pos: self.tokenizer.last_token_pos(),
                program: self.tokenizer.input(),
                found: token,
            })
        }
    }

    /// Set a variable in current or global scope.
    /// If the variable is already defined in the current scope, or the global scope (not: parent scopes!),
    /// it is updated. Otherwise, a new variable is created in the current scope.
    fn set_var(
        &mut self,
        name: &'a [u8],
        value: EngineObject<'a>,
    ) -> Result<(), InterpreterError<'a>> {
        if self.variables.len() >= STACK_SIZE {
            return Err(InterpreterError::VariableStackOverflow);
        }

        // 1. Calculate the number of variables in the current function scope.
        // We iterate backwards through scopes, summing variable counts until we hit a Function boundary.
        let mut locals_count = 0;
        let mut is_inside_function = false;

        for (scope, &count) in self
            .scope_stack
            .iter()
            .zip(self.current_block_scope.iter())
            .rev()
        {
            locals_count += count;
            if matches!(scope, BlockScope::Function { .. }) {
                is_inside_function = true;
                break;
            }
        }

        let stack_len = self.variables.len();
        let locals_range = (stack_len - locals_count..stack_len).rev();

        // 2. Determine global range.
        // We only check globals explicitly if we are currently inside a function.
        // If we are at the top level, `locals_range` already covers the global variables.
        let globals_range = if is_inside_function {
            let global_count = *self.current_block_scope.first().unwrap_or(&0);
            (0..global_count).rev()
        } else {
            (0..0).rev()
        };

        // 3. Perform the search and update
        for i in locals_range.chain(globals_range) {
            if self.variables[i].name == name {
                self.variables[i].value = value;
                return Ok(());
            }
        }

        // 4. Not found: Insert new variable into current scope
        unsafe {
            // SAFETY: checked at function start
            self.variables.push_unchecked(Variable { name, value });
        };

        // Update the count for the current specific block (top of the scope stack)
        if let Some(last) = self.current_block_scope.last_mut() {
            *last += 1;
        }

        Ok(())
    }

    fn get_var(&mut self, name: &'a [u8]) -> Result<&EngineObject<'a>, InterpreterError<'a>> {
        let mut current_stack_index = self.variables.len();

        // Look in current function stack first
        for (scope, &count) in self
            .scope_stack
            .iter()
            .zip(self.current_block_scope.iter())
            .rev()
        {
            let start = current_stack_index - count;
            let end = current_stack_index;
            current_stack_index = start;

            for i in (start..end).rev() {
                if self.variables[i].name == name {
                    return Ok(&self.variables[i].value);
                }
            }

            if let BlockScope::Function { .. } = scope {
                break;
            }
        }

        // Fallback to global context
        if let Some(&global_count) = self.current_block_scope.first() {
            for i in (0..global_count).rev() {
                if self.variables[i].name == name {
                    return Ok(&self.variables[i].value);
                }
            }
        }

        Err(InterpreterError::InvalidName(name))
    }

    /// Ensures that member accesses in modules are resolved
    fn resolve_if_member(
        &mut self,
        mut obj: EngineObject<'a>,
    ) -> Result<EngineObject<'a>, InterpreterError<'a>> {
        while let EngineObject::ModuleMember { module, name } = obj {
            obj = self.modules[module].1.get(name)?;
        }
        Ok(obj)
    }

    /// Core expression evaluator. Returns `Suspended` instead of recursing into user-defined
    /// function bodies. `initial_ops_len` is the operator-stack watermark for this level;
    /// `expect_operand` is true when the next token should be a value/operand.
    fn eval_expr_internal(
        &mut self,
        initial_ops_len: usize,
        mut expect_operand: bool,
    ) -> Result<EvaluationResult<'a>, InterpreterError<'a>> {
        loop {
            self.check_operations_limit()?;
            if expect_operand {
                match self.tokenizer.advance() {
                    Token::BooleanLit(b) => {
                        self.expression_stack
                            .try_push(EngineObject::Bool(b))
                            .map_err(|_| InterpreterError::ExpressionStackOverflow)?;
                        expect_operand = false;
                    }
                    Token::IntegerLit(i) => {
                        self.expression_stack
                            .try_push(EngineObject::Int(i))
                            .map_err(|_| InterpreterError::ExpressionStackOverflow)?;
                        expect_operand = false;
                    }
                    Token::StringLit {
                        content,
                        has_escape_characters,
                    } => {
                        self.expression_stack
                            .try_push(EngineObject::StringLiteral {
                                content,
                                has_escape_characters,
                            })
                            .map_err(|_| InterpreterError::ExpressionStackOverflow)?;
                        expect_operand = false;
                    }
                    Token::Identifier(id) => {
                        let var = self.get_var(id)?.clone();
                        self.expression_stack
                            .try_push(var)
                            .map_err(|_| InterpreterError::ExpressionStackOverflow)?;
                        expect_operand = false;
                    }
                    Token::OpenParen => {
                        self.expression_operator_stack
                            .try_push((Token::OpenParen, 0))
                            .map_err(|_| InterpreterError::ExpressionStackOverflow)?;
                        // expect_operand stays true
                    }
                    Token::Bang => {
                        self.expression_operator_stack
                            .try_push((Token::Bang, 255))
                            .map_err(|_| InterpreterError::ExpressionStackOverflow)?;
                    }
                    Token::Plus => {
                        // unary plus: ignored
                    }
                    Token::Minus => {
                        self.expression_stack
                            .try_push(EngineObject::Int(0))
                            .map_err(|_| InterpreterError::ExpressionStackOverflow)?;
                        self.expression_operator_stack
                            .try_push((Token::Minus, 255))
                            .map_err(|_| InterpreterError::ExpressionStackOverflow)?;
                    }
                    token => {
                        return Err(InterpreterError::InvalidOperandToken {
                            token,
                            token_pos: self.tokenizer.last_token_pos(),
                            program: self.tokenizer.input(),
                        });
                    }
                }
            } else {
                // Expecting an operator
                let op = self.tokenizer.peek();
                match op {
                    Token::Equals
                    | Token::Lt
                    | Token::Gt
                    | Token::Lte
                    | Token::Gte
                    | Token::AndAnd
                    | Token::OrOr
                    | Token::Plus
                    | Token::Minus
                    | Token::Star
                    | Token::Slash
                    | Token::Percent => {
                        let (lbp, rbp) = self
                            .infix_binding_power(&op)
                            .ok_or(InterpreterError::Internal)?;
                        while initial_ops_len < self.expression_operator_stack.len()
                            && let Some((_, top_bp)) = self.expression_operator_stack.last()
                        {
                            if *top_bp >= lbp {
                                self.pop_and_apply()?;
                            } else {
                                break;
                            }
                        }
                        self.consume_token(&op)?;
                        self.expression_operator_stack
                            .try_push((op, rbp))
                            .map_err(|_| InterpreterError::ExpressionStackOverflow)?;
                        expect_operand = true;
                    }
                    Token::Dot => {
                        let lbp = 100;
                        while initial_ops_len < self.expression_operator_stack.len()
                            && let Some((_, top_bp)) = self.expression_operator_stack.last()
                        {
                            if *top_bp >= lbp {
                                self.pop_and_apply()?;
                            } else {
                                break;
                            }
                        }
                        self.consume_token(&Token::Dot)?;
                        self.expression_operator_stack
                            .try_push((Token::Dot, lbp))
                            .map_err(|_| InterpreterError::ExpressionStackOverflow)?;
                        match self.tokenizer.advance() {
                            Token::Identifier(id) => {
                                self.expression_stack
                                    .try_push(EngineObject::MemberAccess { name: id })
                                    .map_err(|_| InterpreterError::ExpressionStackOverflow)?;
                            }
                            t => {
                                return Err(InterpreterError::UnexpectedToken {
                                    token_pos: self.tokenizer.last_token_pos(),
                                    program: self.tokenizer.input(),
                                    expected: Token::Identifier(&[]),
                                    found: t,
                                });
                            }
                        }
                        expect_operand = false;
                    }
                    Token::OpenParen => {
                        // Collapse pending Dot operators so callee is a ModuleMember.
                        while initial_ops_len < self.expression_operator_stack.len() {
                            match self.expression_operator_stack.last() {
                                Some((Token::Dot, _)) => self.pop_and_apply()?,
                                _ => break,
                            }
                        }

                        let Some(function_object) = self.expression_stack.pop() else {
                            return Err(InterpreterError::ExpressionStackEmpty);
                        };

                        self.tokenizer.advance(); // consume '('
                        let args_start = self.expression_stack.len();

                        match function_object {
                            EngineObject::Function { position, num_args } => {
                                // Collect args iteratively; any arg may suspend.
                                let mut nargs_so_far = 0;
                                let mut is_first = true;
                                loop {
                                    if self.tokenizer.peek() == Token::CloseParen {
                                        self.tokenizer.advance();
                                        break;
                                    }
                                    if !is_first {
                                        self.consume_token(&Token::Comma)?;
                                    }
                                    let ops_for_arg = self.expression_operator_stack.len();
                                    match self.eval_expr_internal(ops_for_arg, true)? {
                                        EvaluationResult::Value(v) => {
                                            self.expression_stack.try_push(v).map_err(|_| {
                                                InterpreterError::ExpressionStackOverflow
                                            })?;
                                            nargs_so_far += 1;
                                        }
                                        EvaluationResult::Suspended => {
                                            // Save state for continue_args to resume later.
                                            self.arg_eval_stack
                                                .try_push(FunctionArgState {
                                                    function_object: EngineObject::Function {
                                                        position,
                                                        num_args,
                                                    },
                                                    args_start,
                                                    outer_ops_len: initial_ops_len,
                                                })
                                                .map_err(|_| {
                                                    InterpreterError::ScopeStackExhausted
                                                })?;
                                            return Ok(EvaluationResult::Suspended);
                                        }
                                    }
                                    is_first = false;
                                }

                                // All args collected without suspension.
                                if num_args != nargs_so_far {
                                    return Err(InterpreterError::FunctionArgsMismatch {
                                        expected: num_args,
                                        got: nargs_so_far,
                                        name: None,
                                    });
                                }
                                let return_addr = self.tokenizer.cursor_pos();
                                self.enter_scope(BlockScope::Function {
                                    return_addr,
                                    caller_ops_len: initial_ops_len,
                                })?;
                                self.tokenizer.set_cursor(position);
                                for i in 0..num_args {
                                    let arg_name = match self.tokenizer.advance() {
                                        Token::Identifier(n) => n,
                                        t => {
                                            return Err(InterpreterError::UnexpectedToken {
                                                token_pos: self.tokenizer.last_token_pos(),
                                                program: self.tokenizer.input(),
                                                expected: Token::Identifier(&[]),
                                                found: t,
                                            });
                                        }
                                    };
                                    let value = core::mem::replace(
                                        &mut self.expression_stack[args_start + i],
                                        EngineObject::Unit,
                                    );
                                    self.set_var(arg_name, value)?;
                                    if i < num_args - 1 {
                                        self.consume_token(&Token::Comma)?;
                                    }
                                }
                                self.expression_stack.truncate(args_start);
                                self.consume_token(&Token::CloseParen)?;
                                self.consume_token(&Token::OpenBrace)?;
                                return Ok(EvaluationResult::Suspended);
                            }
                            EngineObject::ModuleMember { module: idx, name } => {
                                // Module calls are synchronous — collect all args then call.
                                let mut nargs_so_far = 0;
                                let mut is_first = true;
                                loop {
                                    if self.tokenizer.peek() == Token::CloseParen {
                                        self.tokenizer.advance();
                                        break;
                                    }
                                    if !is_first {
                                        self.consume_token(&Token::Comma)?;
                                    }
                                    let ops_for_arg = self.expression_operator_stack.len();
                                    match self.eval_expr_internal(ops_for_arg, true)? {
                                        EvaluationResult::Value(v) => {
                                            self.expression_stack.try_push(v).map_err(|_| {
                                                InterpreterError::ExpressionStackOverflow
                                            })?;
                                            nargs_so_far += 1;
                                        }
                                        EvaluationResult::Suspended => {
                                            self.arg_eval_stack
                                                .try_push(FunctionArgState {
                                                    function_object: EngineObject::ModuleMember {
                                                        module: idx,
                                                        name,
                                                    },
                                                    args_start,
                                                    outer_ops_len: initial_ops_len,
                                                })
                                                .map_err(|_| {
                                                    InterpreterError::ScopeStackExhausted
                                                })?;
                                            return Ok(EvaluationResult::Suspended);
                                        }
                                    }
                                    is_first = false;
                                }
                                let result = {
                                    let args = &self.expression_stack
                                        [args_start..args_start + nargs_so_far];
                                    self.modules[idx].1.call(name, args)?
                                };
                                self.expression_stack.truncate(args_start);
                                self.expression_stack
                                    .try_push(result)
                                    .map_err(|_| InterpreterError::ExpressionStackOverflow)?;
                                expect_operand = false;
                            }
                            obj => {
                                return Err(InterpreterError::InvalidFunctionCall { obj });
                            }
                        }
                    }
                    Token::CloseParen => {
                        let has_matching_paren = self.expression_operator_stack[initial_ops_len..]
                            .iter()
                            .any(|(t, _)| *t == Token::OpenParen);

                        if !has_matching_paren {
                            break;
                        }

                        while initial_ops_len < self.expression_operator_stack.len()
                            && let Some((top_op, _)) = self.expression_operator_stack.last()
                        {
                            if *top_op == Token::OpenParen {
                                break;
                            }
                            self.pop_and_apply()?;
                        }

                        self.expression_operator_stack.pop(); // pop the OpenParen sentinel
                        self.consume_token(&Token::CloseParen)?;
                        expect_operand = false;
                    }
                    _ => break,
                }
            }
        }

        while initial_ops_len < self.expression_operator_stack.len() {
            self.pop_and_apply()?;
        }

        let res = self
            .expression_stack
            .pop()
            .ok_or(InterpreterError::ExpressionStackEmpty)?;

        Ok(EvaluationResult::Value(self.resolve_if_member(res)?))
    }

    /// Called after a suspended arg's inner function has returned and its result has been
    /// placed on the expression_stack by the resume path. Continues collecting args for the
    /// outer function whose state is on top of `arg_eval_stack`, then sets up the call.
    fn continue_args(
        &mut self,
        completed_arg: EngineObject<'a>,
    ) -> Result<EvaluationResult<'a>, InterpreterError<'a>> {
        let args_start = self
            .arg_eval_stack
            .last()
            .ok_or(InterpreterError::Internal)?
            .args_start;

        // Push the just-finished arg.
        self.expression_stack
            .try_push(completed_arg)
            .map_err(|_| InterpreterError::ExpressionStackOverflow)?;

        // Collect remaining args.
        loop {
            if self.tokenizer.peek() == Token::CloseParen {
                self.tokenizer.advance();
                break;
            }
            self.consume_token(&Token::Comma)?;
            let ops_for_arg = self.expression_operator_stack.len();
            match self.eval_expr_internal(ops_for_arg, true)? {
                EvaluationResult::Value(v) => {
                    self.expression_stack
                        .try_push(v)
                        .map_err(|_| InterpreterError::ExpressionStackOverflow)?;
                }
                EvaluationResult::Suspended => {
                    return Ok(EvaluationResult::Suspended);
                }
            }
        }

        // All args collected — set up the call.
        let state = self
            .arg_eval_stack
            .pop()
            .ok_or(InterpreterError::Internal)?;
        let nargs_collected = self.expression_stack.len() - args_start;
        let return_addr = self.tokenizer.cursor_pos(); // after ')'
        let outer_ops_len = state.outer_ops_len;

        match state.function_object {
            EngineObject::Function { position, num_args } => {
                if num_args != nargs_collected {
                    return Err(InterpreterError::FunctionArgsMismatch {
                        expected: num_args,
                        got: nargs_collected,
                        name: None,
                    });
                }
                self.enter_scope(BlockScope::Function {
                    return_addr,
                    caller_ops_len: outer_ops_len,
                })?;
                self.tokenizer.set_cursor(position);
                for i in 0..num_args {
                    let arg_name = match self.tokenizer.advance() {
                        Token::Identifier(n) => n,
                        t => {
                            return Err(InterpreterError::UnexpectedToken {
                                token_pos: self.tokenizer.last_token_pos(),
                                program: self.tokenizer.input(),
                                expected: Token::Identifier(&[]),
                                found: t,
                            });
                        }
                    };
                    let value = core::mem::replace(
                        &mut self.expression_stack[args_start + i],
                        EngineObject::Unit,
                    );
                    self.set_var(arg_name, value)?;
                    if i < num_args - 1 {
                        self.consume_token(&Token::Comma)?;
                    }
                }
                self.expression_stack.truncate(args_start);
                self.consume_token(&Token::CloseParen)?;
                self.consume_token(&Token::OpenBrace)?;
                Ok(EvaluationResult::Suspended)
            }
            EngineObject::ModuleMember { module: idx, name } => {
                let result = {
                    let args = &self.expression_stack[args_start..args_start + nargs_collected];
                    self.modules[idx].1.call(name, args)?
                };
                self.expression_stack.truncate(args_start);
                Ok(EvaluationResult::Value(result))
            }
            obj => Err(InterpreterError::InvalidFunctionCall { obj }),
        }
    }

    /// Dispatch the result of a completed expression to the appropriate continuation.
    /// Note: callers that resume from a function return should drain `arg_eval_stack`
    /// themselves before calling this (see the resume path in `step()`).
    fn handle_evaluation_result(
        &mut self,
        result: EvaluationResult<'a>,
    ) -> Result<bool, InterpreterError<'a>> {
        match result {
            EvaluationResult::Suspended => Ok(true),
            EvaluationResult::Value(value) => {
                // Pop the continuation scope.
                let Some(cont) = self.scope_stack.pop() else {
                    return Err(InterpreterError::ScopeStackEmpty);
                };
                let _var_count = self
                    .current_block_scope
                    .pop()
                    .ok_or(InterpreterError::ScopeStackEmpty)?;
                // Continuation scopes always have 0 variables, so _var_count == 0.

                match cont {
                    BlockScope::Assignment { name } => {
                        self.set_var(name, value)?;
                        self.consume_separator()?;
                        Ok(true)
                    }
                    BlockScope::ExpressionStatement => {
                        // value is discarded
                        self.consume_separator()?;
                        Ok(true)
                    }
                    BlockScope::Return => {
                        // Unwind scope stack to the enclosing Function scope.
                        let mut vars_to_remove = 0;
                        let (return_addr, caller_ops_len) = loop {
                            let Some(scope) = self.scope_stack.pop() else {
                                return Err(InterpreterError::ScopeStackEmpty);
                            };
                            vars_to_remove += self
                                .current_block_scope
                                .pop()
                                .ok_or(InterpreterError::ScopeStackEmpty)?;
                            if let BlockScope::Function {
                                return_addr,
                                caller_ops_len,
                            } = scope
                            {
                                break (return_addr, caller_ops_len);
                            }
                        };
                        self.variables
                            .truncate(self.variables.len() - vars_to_remove);
                        self.expression_stack
                            .try_push(value)
                            .map_err(|_| InterpreterError::ExpressionStackOverflow)?;
                        self.tokenizer.set_cursor(return_addr);
                        self.resume_expression = Some(caller_ops_len);
                        Ok(true)
                    }
                    BlockScope::IfCondition => {
                        let cond = value.is_true()?;
                        self.consume_token(&Token::OpenBrace)?;
                        if cond {
                            self.enter_scope(BlockScope::If)?;
                        } else {
                            self.skip_block(None)?;
                            let next = self.tokenizer.peek();
                            if next != Token::Else {
                                self.consume_separator()?;
                            } else {
                                self.consume_token(&Token::Else)?;
                                self.consume_token(&Token::OpenBrace)?;
                                self.enter_scope(BlockScope::Else)?;
                            }
                        }
                        Ok(true)
                    }
                    BlockScope::WhileCondition { condition_start } => {
                        let cond = value.is_true()?;
                        self.consume_token(&Token::OpenBrace)?;
                        if cond {
                            self.enter_scope(BlockScope::While { condition_start })?;
                        } else {
                            self.skip_block(None)?;
                            self.consume_separator()?;
                        }
                        Ok(true)
                    }
                    _ => Err(InterpreterError::ScopeStackEmpty),
                }
            }
        }
    }

    /// Pops one operator and the necessary number of operands, applies the operator, and pushes the result.
    fn pop_and_apply(&mut self) -> Result<(), InterpreterError<'a>> {
        let (op, _) = self
            .expression_operator_stack
            .pop()
            .ok_or(InterpreterError::Internal)?;
        let right = self
            .expression_stack
            .pop()
            .ok_or(InterpreterError::ExpressionStackEmpty)?;
        let right = self.resolve_if_member(right)?;

        if op == Token::Bang {
            // Note that minus is handled as 0 - right, so no need to handle it here
            // Also note that plus is allowed, but ignored as unary operator
            return match right {
                EngineObject::Bool(b) => self
                    .expression_stack
                    .try_push(EngineObject::Bool(!b))
                    .map_err(|_| InterpreterError::ExpressionStackOverflow),
                EngineObject::Int(i) => self
                    .expression_stack
                    .try_push(EngineObject::Int(if i == 0 { 1 } else { 0 }))
                    .map_err(|_| InterpreterError::ExpressionStackOverflow),
                _ => Err(InterpreterError::InvalidUnaryOperation {
                    op,
                    obj: right,
                    program: self.tokenizer.input(),
                    token_pos: self.tokenizer.last_token_pos(),
                }),
            };
        }

        let left = self
            .expression_stack
            .pop()
            .ok_or(InterpreterError::ExpressionStackEmpty)?;
        let mut left = self.resolve_if_member(left)?;

        match (&mut left, op, &right) {
            // Integer math
            (EngineObject::Int(l), Token::Plus, EngineObject::Int(r)) => {
                *l = l
                    .checked_add(*r)
                    .ok_or(InterpreterError::OperatorOverflow {
                        op,
                        program: self.tokenizer.input(),
                        token_pos: self.tokenizer.last_token_pos(),
                    })?;
            }
            (EngineObject::Int(l), Token::Minus, EngineObject::Int(r)) => {
                *l = l
                    .checked_sub(*r)
                    .ok_or(InterpreterError::OperatorOverflow {
                        op,
                        program: self.tokenizer.input(),
                        token_pos: self.tokenizer.last_token_pos(),
                    })?;
            }
            (EngineObject::Int(l), Token::Star, EngineObject::Int(r)) => {
                *l = l
                    .checked_mul(*r)
                    .ok_or(InterpreterError::OperatorOverflow {
                        op,
                        program: self.tokenizer.input(),
                        token_pos: self.tokenizer.last_token_pos(),
                    })?;
            }
            (EngineObject::Int(l), Token::Slash, EngineObject::Int(r)) => {
                if *r == 0 {
                    return Err(InterpreterError::DivisionByZero);
                }
                *l = l
                    .checked_div(*r)
                    .ok_or(InterpreterError::OperatorOverflow {
                        op,
                        program: self.tokenizer.input(),
                        token_pos: self.tokenizer.last_token_pos(),
                    })?;
            }
            (EngineObject::Int(l), Token::Percent, EngineObject::Int(r)) => {
                if *r == 0 {
                    return Err(InterpreterError::DivisionByZero);
                }
                *l = l
                    .checked_rem(*r)
                    .ok_or(InterpreterError::OperatorOverflow {
                        op,
                        program: self.tokenizer.input(),
                        token_pos: self.tokenizer.last_token_pos(),
                    })?;
            }

            // Comparison operators
            (l, Token::Equals, r) => left = EngineObject::Bool(*l == *r),
            (l, Token::NotEquals, r) => left = EngineObject::Bool(*l != *r),

            // Integer ops
            (EngineObject::Int(l), Token::Lt, EngineObject::Int(r)) => {
                left = EngineObject::Bool(*l < *r)
            }
            (EngineObject::Int(l), Token::Gt, EngineObject::Int(r)) => {
                left = EngineObject::Bool(*l > *r)
            }
            (EngineObject::Int(l), Token::Lte, EngineObject::Int(r)) => {
                left = EngineObject::Bool(*l <= *r)
            }
            (EngineObject::Int(l), Token::Gte, EngineObject::Int(r)) => {
                left = EngineObject::Bool(*l >= *r)
            }

            // Boolean operations
            (EngineObject::Bool(l), Token::AndAnd, EngineObject::Bool(r)) => {
                left = EngineObject::Bool(*l && *r)
            }
            (EngineObject::Bool(l), Token::OrOr, EngineObject::Bool(r)) => {
                left = EngineObject::Bool(*l || *r)
            }

            // Handle Dot Access
            (EngineObject::Module(idx), Token::Dot, EngineObject::MemberAccess { name }) => {
                left = EngineObject::ModuleMember { module: *idx, name };
            }

            // Error
            _ => {
                return Err(InterpreterError::InvalidBinaryOperation {
                    op,
                    left: left.clone(),
                    right: right.clone(),
                    program: self.tokenizer.input(),
                    token_pos: self.tokenizer.last_token_pos(),
                });
            }
        }

        self.expression_stack
            .try_push(left)
            .map_err(|_| InterpreterError::ExpressionStackOverflow)?;
        Ok(())
    }

    const fn infix_binding_power(&self, token: &Token) -> Option<(u8, u8)> {
        match token {
            Token::Plus | Token::Minus => Some((2, 3)), // left bp, right bp
            Token::Star | Token::Slash | Token::Percent => Some((4, 5)),
            Token::NotEquals | Token::Equals | Token::Lt | Token::Gt | Token::Lte | Token::Gte => {
                Some((1, 2))
            }
            Token::AndAnd | Token::OrOr => Some((1, 2)),
            Token::Dot => Some((100, 101)),
            Token::Assign => Some((0, 1)),
            _ => None,
        }
    }

    fn skip_block(&mut self, initial: Option<usize>) -> Result<(), InterpreterError<'a>> {
        let mut depth = initial.unwrap_or(1); // We assume we just passed the opening '{' (or are about to)

        while depth > 0 {
            match self.tokenizer.advance() {
                Token::OpenBrace => depth += 1,
                Token::CloseBrace => depth -= 1,
                Token::Eof => return Err(InterpreterError::UnexpectedEoF),
                _ => {} // Ignore everything else
            }
        }
        Ok(())
    }
}

impl<
    'm,
    const STACK_SIZE: usize,
    const MAX_SCOPE_DEPTH: usize,
    const MAX_EXPRESSION_DEPTH: usize,
    const MAX_MODULES: usize,
> Default for VmContext<'m, STACK_SIZE, MAX_SCOPE_DEPTH, MAX_EXPRESSION_DEPTH, MAX_MODULES>
{
    fn default() -> Self {
        Self::new()
    }
}

impl<
    'm,
    const STACK_SIZE: usize,
    const MAX_SCOPE_DEPTH: usize,
    const MAX_EXPRESSION_DEPTH: usize,
    const MAX_MODULES: usize,
> VmContext<'m, STACK_SIZE, MAX_SCOPE_DEPTH, MAX_EXPRESSION_DEPTH, MAX_MODULES>
{
    const _ASSERT_STACK_SIZE: () = assert!(STACK_SIZE > 0, "STACK_SIZE must be greater than 0");
    const _ASSERT_MAX_SCOPE_DEPTH: () = assert!(
        MAX_SCOPE_DEPTH > 0,
        "MAX_SCOPE_DEPTH must be greater than 0"
    );

    /// Create a new VM context with no modules registered and no operations limit.
    ///
    /// ```rust
    /// use nova::{VmContext, EngineObject};
    ///
    /// let mut vm: VmContext<'_> = VmContext::new();
    /// let result = vm.run(b"n = 1 + 1;").unwrap();
    /// assert_eq!(result.get_var(b"n"), Some(&EngineObject::Int(2)));
    /// ```
    pub const fn new() -> Self {
        Self {
            modules: ArrayVec::new_const(),
            operations_limit: usize::MAX,
        }
    }

    /// Limit the total number of operations a single [`run`](Self::run) call may perform.
    ///
    /// This is the primary way to protect against infinite loops or runaway scripts.
    /// Each token evaluation counts as one operation.
    ///
    /// ```rust
    /// use nova::{VmContext, InterpreterError};
    ///
    /// let mut vm: VmContext<'_> = VmContext::new();
    /// vm.set_operations_limit(500);
    /// assert!(matches!(
    ///     vm.run(b"i = 0; while true { i = i + 1; }"),
    ///     Err(InterpreterError::TooManyOperations),
    /// ));
    /// ```
    pub fn set_operations_limit(&mut self, limit: usize) {
        self.operations_limit = limit;
    }

    /// Register a module under `name`, available in scripts via `import <name>`.
    ///
    /// Uses a builder pattern so modules can be chained before the first [`run`](Self::run).
    ///
    /// Returns [`InterpreterError::TooManyModules`] if `MAX_MODULES` is already reached.
    ///
    /// ```rust
    /// use nova::{VmContext, engine_module, script_module, FromEngine};
    ///
    /// #[engine_module]
    /// struct Calc;
    ///
    /// #[script_module]
    /// impl Calc {
    ///     pub fn double(&self, x: i32) -> i32 { x * 2 }
    /// }
    ///
    /// let mut calc = Calc {};
    /// let mut vm: VmContext<'_> = VmContext::new().add_module(b"calc", &mut calc).unwrap();
    /// let result = vm.run(b"import calc; y = calc.double(21);").unwrap();
    /// let y: i32 = FromEngine::from_engine(result.get_var(b"y").unwrap()).unwrap();
    /// assert_eq!(y, 42);
    /// ```
    pub fn add_module(
        mut self,
        name: &'m [u8],
        m: &'m mut dyn Module,
    ) -> Result<Self, InterpreterError<'static>> {
        self.modules
            .try_push((name, m))
            .map_err(|_| InterpreterError::TooManyModules)?;
        Ok(self)
    }

    /// Execute `script`, returning a [`RunResult`] for inspecting global variables.
    ///
    /// The same `VmContext` can be reused across multiple calls; registered modules
    /// and the operations limit persist, but all script state (variables, call stack)
    /// is discarded between runs.
    ///
    /// ```rust
    /// use nova::{VmContext, EngineObject};
    ///
    /// let mut vm: VmContext<'_> = VmContext::new();
    ///
    /// // First run — defines and calls a function
    /// let r = vm.run(br#"
    ///     fn fib(n) {
    ///         if n <= 1 { return n; }
    ///         return fib(n - 1) + fib(n - 2);
    ///     }
    ///     result = fib(10);
    /// "#).unwrap();
    /// assert_eq!(r.get_var(b"result"), Some(&EngineObject::Int(55)));
    ///
    /// // Second run with a different script — previous variables are gone
    /// let r2 = vm.run(b"x = 99;").unwrap();
    /// assert_eq!(r2.get_var(b"result"), None);
    /// assert_eq!(r2.get_var(b"x"), Some(&EngineObject::Int(99)));
    /// ```
    pub fn run<'a>(
        &mut self,
        script: &'a [u8],
    ) -> Result<RunResult<'a, STACK_SIZE>, InterpreterError<'a>> {
        let mut exec: Execution<
            'a,
            '_,
            'm,
            STACK_SIZE,
            MAX_SCOPE_DEPTH,
            MAX_EXPRESSION_DEPTH,
            MAX_MODULES,
        > = Execution {
            variables: ArrayVec::new_const(),
            scope_stack: ArrayVec::new_const(),
            current_block_scope: ArrayVec::new_const(),
            expression_stack: ArrayVec::new_const(),
            expression_operator_stack: ArrayVec::new_const(),
            resume_expression: None,
            arg_eval_stack: ArrayVec::new_const(),
            tokenizer: Tokenizer::new(script),
            modules: &mut self.modules,
            operations_limit: self.operations_limit,
            current_operations: 0,
        };
        // global context starts with 0 objects
        unsafe {
            // SAFETY: works since MAX_SCOPE_DEPTH > 0, asserted above
            // We use BlockScope::Normal to represent the global scope
            exec.scope_stack.push_unchecked(BlockScope::Normal);
            exec.current_block_scope.push_unchecked(0);
        }
        exec.run_loop()?;
        Ok(exec.into_result())
    }
}

// Stack-frame size guarantees for the four predefined tiers.
// Each assertion verifies that a single `run()` call allocates at most the
// advertised amount for the `Execution` state (VmContext itself is much smaller).
const _: () = assert!(
    core::mem::size_of::<Execution<'static, 'static, 'static, 4, 4, 4, 2>>() <= 1024,
    "VmContextTiny Execution must fit in 1 KB"
);
const _: () = assert!(
    core::mem::size_of::<Execution<'static, 'static, 'static, 8, 8, 8, 2>>() <= 2048,
    "VmContextSmall Execution must fit in 2 KB"
);
const _: () = assert!(
    core::mem::size_of::<Execution<'static, 'static, 'static, 16, 16, 16, 4>>() <= 4096,
    "VmContextMedium Execution must fit in 4 KB"
);
const _: () = assert!(
    core::mem::size_of::<Execution<'static, 'static, 'static, 32, 32, 16, 4>>() <= 8192,
    "VmContextLarge Execution must fit in 8 KB"
);

#[cfg(test)]
mod tests {
    use super::*;

    use similar_asserts::assert_eq;

    fn assert_expr(input: &[u8], expected: EngineObject) {
        let code = [b"result = ".as_slice(), input].concat();
        let mut vm: VmContext<'_> = VmContext::new();
        let result = vm.run(&code).unwrap();
        assert_eq!(result.get_var(b"result").unwrap(), &expected);
    }

    #[test]
    fn test_simple_expr() {
        assert_expr(b"1 + 2 * 3", 7.to_engine().unwrap());
    }

    #[test]
    fn test_simple_expr2() {
        assert_expr(b"2 * 3 + 1", 7.to_engine().unwrap());
    }

    #[test]
    fn long_expression() {
        assert_expr(b"1 + 2 + 3 * 8 + 4 + 5", 36.to_engine().unwrap());
    }

    #[test]
    fn simple_parens_expression() {
        assert_expr(b"(1 + 2 + 3)", 6.to_engine().unwrap());
    }
    #[test]
    fn parens_expression() {
        assert_expr(b"(1 + 2 + 3) * (8 + 4 + 5)", 102.to_engine().unwrap());
    }

    #[test]
    fn parens_nested() {
        assert_expr(b"((1 + 2) * (3 + 4)) * 5", 105.to_engine().unwrap());
    }

    #[test]
    fn parens_nested2() {
        assert_expr(b"(1 * (2 * (3 * 4))) * 5", 120.to_engine().unwrap());
    }

    #[test]
    fn parens_nested3() {
        assert_expr(b"5 * (4 * (3 * (2 * 1)))", 120.to_engine().unwrap());
    }

    #[test]
    fn comparison_operators() {
        assert_expr(b"(1 < 8)", true.to_engine().unwrap());
    }

    #[test]
    fn unary_operators() {
        assert_expr(b"-5", (-5).to_engine().unwrap());
    }
    #[test]
    fn unary_operators2() {
        assert_expr(b"!5", 0.to_engine().unwrap());
    }
    #[test]
    fn unary_operators3() {
        assert_expr(b"!0", 1.to_engine().unwrap());
    }

    #[test]
    fn assign_variables() {
        let mut vm: VmContext<'_> = VmContext::new();
        let result = vm.run(b"a = 5 + 5;").unwrap();
        assert_eq!(*result.get_var(b"a").unwrap(), 10.to_engine().unwrap());
    }

    #[test]
    fn assign_multiple_variables() {
        let mut vm: VmContext<'_> = VmContext::new();
        let result = vm.run(b"a = 5 + 5; b = a + 5;").unwrap();
        assert_eq!(*result.get_var(b"a").unwrap(), 10.to_engine().unwrap());
        assert_eq!(*result.get_var(b"b").unwrap(), 15.to_engine().unwrap());
    }

    #[test]
    fn assign_too_many_variables() {
        // Limit stack to at most 2 variables
        let mut vm: VmContext<'_, 2, 16, 8, 16> = VmContext::new();
        assert!(matches!(
            vm.run(b"a = 5 + 5; b = a + 5; c = b + a;"),
            Err(InterpreterError::VariableStackOverflow)
        ));
    }

    #[test]
    fn declare_function() {
        let mut vm: VmContext<'_> = VmContext::new();
        let result = vm
            .run(
                br#"fn test(a, b) { return a + b }
            fn test2() { return 7 }
        "#,
            )
            .expect("Running VM to declare function");

        let test_func = result.get_var(b"test").expect("function to be variable");
        assert!(matches!(
            test_func,
            EngineObject::Function {
                position: 8,
                num_args: 2,
            }
        ));

        let test_func2 = result.get_var(b"test2").expect("function to be variable");
        assert!(matches!(
            test_func2,
            EngineObject::Function {
                position: 52,
                num_args: 0,
            }
        ));
    }

    #[test]
    fn if_else() {
        let mut vm: VmContext<'_> = VmContext::new();
        let result = vm
            .run(
                br#"a = 5;
            b = 0;
            if a > 3 {
                b = 10;
            } else {
                b = 20;
            }
        "#,
            )
            .expect("Running VM with if-else");
        assert_eq!(*result.get_var(b"b").unwrap(), 10.to_engine().unwrap());
    }

    #[test]
    fn if_only() {
        let mut vm: VmContext<'_> = VmContext::new();
        let result = vm
            .run(
                br#"a = 5;
            b = 0;
            if a > 3 {
                b = 10;
            }
        "#,
            )
            .expect("Running VM with if-else");
        assert_eq!(*result.get_var(b"b").unwrap(), 10.to_engine().unwrap());
    }

    #[test]
    fn if_nested() {
        let mut vm: VmContext<'_> = VmContext::new();
        let result = vm
            .run(
                br#"a = 5;
            b = 0;
            if a > 3 {
                if (a < 10) {
                    b = 10;
                }
            }
        "#,
            )
            .expect("Running VM with if-else");
        assert_eq!(*result.get_var(b"b").unwrap(), 10.to_engine().unwrap());
    }

    #[test]
    fn function_call() {
        let mut vm: VmContext<'_> = VmContext::new();
        let result = vm
            .run(
                br#"fn val() { return 5; }
                c = val();
            "#,
            )
            .expect("Running VM with function call");
        assert_eq!(*result.get_var(b"c").unwrap(), 5.to_engine().unwrap());
    }

    #[test]
    fn stack_overflow() {
        let mut vm: VmContext<'_> = VmContext::new();
        assert!(matches!(
            vm.run(
                br#"fn recurse() { return recurse(); }
                recurse();
            "#,
            ),
            Err(InterpreterError::ScopeStackExhausted)
        ));
    }

    #[test]
    fn fibonacci() {
        let mut vm: VmContext<'_> = VmContext::new();
        let result = vm
            .run(
                br#"fn fib(n) {
                if n <= 1 {
                    return n;
                } else {
                    return fib(n - 1) + fib(n - 2);
                }
            }
            result = fib(10);
            "#,
            )
            .expect("Running VM with Fibonacci function");
        assert_eq!(*result.get_var(b"result").unwrap(), 55.to_engine().unwrap());
    }

    #[test]
    fn function_call_two_args() {
        let mut vm: VmContext<'_> = VmContext::new();
        let result = vm
            .run(b"fn add(a, b) { return a + b; }\nresult = add(2, 3);")
            .unwrap();
        assert_eq!(*result.get_var(b"result").unwrap(), 5.to_engine().unwrap());
    }

    #[test]
    fn function_call_implicit_unit() {
        let mut vm: VmContext<'_> = VmContext::new();
        let result = vm.run(b"fn noop() {}\nresult = noop();").unwrap();
        assert_eq!(*result.get_var(b"result").unwrap(), EngineObject::Unit);
    }

    #[test]
    fn function_call_in_expression() {
        let mut vm: VmContext<'_> = VmContext::new();
        let result = vm
            .run(b"fn add(a, b) { return a + b; }\nresult = add(2, 3) + 1;")
            .unwrap();
        assert_eq!(*result.get_var(b"result").unwrap(), 6.to_engine().unwrap());
    }

    #[test]
    fn nested_function_call_as_arg() {
        let mut vm: VmContext<'_> = VmContext::new();
        let result = vm.run(b"fn double(x) { return x * 2; }\nfn add(a, b) { return a + b; }\nresult = add(double(2), 3);").unwrap();
        assert_eq!(*result.get_var(b"result").unwrap(), 7.to_engine().unwrap());
    }

    #[test]
    fn while_loop_basic() {
        let mut vm: VmContext<'_> = VmContext::new();
        let result = vm.run(b"i = 0;\nwhile i < 3 {\ni = i + 1;\n}").unwrap();
        assert_eq!(*result.get_var(b"i").unwrap(), 3.to_engine().unwrap());
    }

    #[test]
    fn while_loop_never_executes() {
        let mut vm: VmContext<'_> = VmContext::new();
        let result = vm.run(b"i = 5;\nwhile i < 3 {\ni = i + 1;\n}").unwrap();
        assert_eq!(*result.get_var(b"i").unwrap(), 5.to_engine().unwrap());
    }

    #[test]
    fn while_loop_accumulates() {
        let mut vm: VmContext<'_> = VmContext::new();
        let result = vm
            .run(b"i = 0;\nsum = 0;\nwhile i < 5 {\nsum = sum + i;\ni = i + 1;\n}")
            .unwrap();
        assert_eq!(*result.get_var(b"sum").unwrap(), 10.to_engine().unwrap());
    }

    #[test]
    fn while_loop_continue() {
        let mut vm: VmContext<'_> = VmContext::new();
        let result = vm
            .run(
                br"
            i = 0;
            sum = 0;
            while i < 4 {
                if i == 2 {
                    i = i + 1;
                    continue;
                }
                sum = sum + i;
                i = i + 1;
            }",
            )
            .unwrap();
        assert_eq!(*result.get_var(b"sum").unwrap(), 4.to_engine().unwrap());
        assert_eq!(*result.get_var(b"i").unwrap(), 4.to_engine().unwrap());
    }

    #[test]
    fn while_loop_break() {
        let mut vm: VmContext<'_> = VmContext::new();
        let result = vm
            .run(
                br"
            i = 0;
            sum = 0;
            while i < 4 {
                if i == 2 {
                    break;
                }
                sum = sum + i;
                i = i + 1;
            }",
            )
            .unwrap();
        assert_eq!(*result.get_var(b"sum").unwrap(), 1.to_engine().unwrap());
        assert_eq!(*result.get_var(b"i").unwrap(), 2.to_engine().unwrap());
    }

    #[test]
    fn while_loop_nested() {
        let mut vm: VmContext<'_> = VmContext::new();
        let result = vm
            .run(
                br"
            i = 0;
            sum = 0;
            while i < 3 {
                j = 0;
                tmp = i + 1;
                while j < 2 {
                    new_sum = sum + i + j;
                    sum = new_sum;
                    j = j + 1;
                }
                i = tmp;
            }",
            )
            .unwrap();
        assert_eq!(*result.get_var(b"sum").unwrap(), 9.to_engine().unwrap());
    }

    #[test]
    fn continue_outside_loop() {
        let mut vm: VmContext<'_> = VmContext::new();
        assert!(matches!(
            vm.run(b"continue;"),
            Err(InterpreterError::ContinueOutsideLoop)
        ));
    }

    #[test]
    fn break_outside_loop() {
        let mut vm: VmContext<'_> = VmContext::new();
        assert!(matches!(
            vm.run(b"if true { break; }"),
            Err(InterpreterError::BreakOutsideLoop)
        ));
    }

    #[test]
    fn fibonacci_two() {
        let mut vm: VmContext<'_> = VmContext::new();
        let result = vm
            .run(
                br#"
            fn fib_recursive(n) {
                if n == 0 {
                    return 0;
                }
                if n == 1 {
                    return 1;
                }
                return fib_recursive(n-1) + fib_recursive(n-2);
            };

            fn fib_iterative(n) {
                a = 0;
                b = 1;
                if n == 0 {
                    return a;
                }
                if n == 1 {
                    return b;
                }
                i = 2;
                while i <= n {
                    c = a + b;
                    a = b;
                    b = c;
                    i = i + 1;
                }
                return b;
            };

            it_res = fib_iterative(10);
            rc_res = fib_recursive(10);

            same = it_res == rc_res;
            "#,
            )
            .expect("Running VM with Fibonacci function");
        assert_eq!(*result.get_var(b"it_res").unwrap(), 55.to_engine().unwrap());
        assert_eq!(*result.get_var(b"rc_res").unwrap(), 55.to_engine().unwrap());
        assert_eq!(*result.get_var(b"same").unwrap(), true.to_engine().unwrap());
    }

    #[test]
    fn mul_overflow() {
        let mut vm: VmContext<'_> = VmContext::new();
        assert!(matches!(
            vm.run(
                br#"
            n = 25;
            while true {
                n = n * 25;
            }
            "#,
            ),
            Err(InterpreterError::OperatorOverflow {
                op: Token::Star,
                ..
            })
        ));
    }

    #[test]
    fn test_sibling() {
        let mut vm: VmContext<'_, 32, 32> = VmContext::new();
        let result = vm
            .run(
                br#"
            fn is_even(n) {
                if n == 0 {
                    return true;
                } else {
                    return is_odd(n - 1);
                }
            }

            fn is_odd(n) {
                return is_even(n - 1);
            }

            result = is_even(4);
            "#,
            )
            .expect("Running VM with sibling functions");
        assert_eq!(
            *result.get_var(b"result").unwrap(),
            true.to_engine().unwrap()
        );
    }

    #[test]
    fn test_sibling_stack_overflow() {
        let mut vm: VmContext<'_, 32, 32> = VmContext::new();
        assert!(matches!(
            vm.run(
                br#"
            fn is_even(n) {
                if n == 0 {
                    return true;
                } else {
                    return is_odd(n - 1);
                }
            }

            fn is_odd(n) {
                return is_even(n - 1);
            }

            result = is_even(5);
            "#,
            ),
            Err(InterpreterError::ScopeStackExhausted)
        ));
    }

    #[test]
    fn test_collatz() {
        let mut vm: VmContext<'_> = VmContext::new();
        let result = vm
            .run(
                br#"
            fn computeChainLength(n) {
                steps = 0;
                while n > 1 {
                    if n == 1 {
                        break;
                    }
                    if n % 2 == 0 {
                        n = n / 2;
                    } else {
                        n = 3 * n + 1;
                    }
                    steps = steps + 1;
                }
                return steps;
            }

            i = 1;
            max_length = 0;
            max_number = 0;
            while i < 1000 {
                current_length = computeChainLength(i);
                if current_length > max_length {
                    max_length = current_length;
                    max_number = i;
                }
                i = i + 1;
            }
            "#,
            )
            .expect("Running VM with Collatz function");
        assert_eq!(
            *result.get_var(b"max_number").unwrap(),
            871.to_engine().unwrap()
        );
    }

    #[test]
    fn test_boolean_operators() {
        let mut vm: VmContext<'_> = VmContext::new();
        let result = vm
            .run(
                br#"
            a = true;
            b = false;
            c = a && b;
            d = a || b;
            e = !a;
            "#,
            )
            .expect("Running VM with boolean operators");
        assert_eq!(*result.get_var(b"c").unwrap(), false.to_engine().unwrap());
        assert_eq!(*result.get_var(b"d").unwrap(), true.to_engine().unwrap());
        assert_eq!(*result.get_var(b"e").unwrap(), false.to_engine().unwrap());
    }

    #[test]
    fn test_stack_overflow() {
        let mut vm: VmContext<'_, 32, 32> = VmContext::new();
        assert!(matches!(
            vm.run(
                br#"
            fn is_even(n) {
                return is_even(n+1);
            }
            is_even(0);
            "#,
            ),
            Err(InterpreterError::ScopeStackExhausted)
        ));
    }

    #[test]
    fn test_crash() {
        let mut vm: VmContext<'_, 16, 16> = VmContext::new();
        let result = vm.run(br#"6666666666&"#);
        assert!(matches!(result, Err(_)));
    }
}
