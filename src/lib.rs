#![cfg_attr(not(test), no_std)]

#[cfg(any(debug_assertions, test))]
use core::fmt::Debug;

use arrayvec::ArrayVec;

use crate::tokenizer::{Token, Tokenizer};

pub use nova_macros::{engine_module, script_module};

mod tokenizer;

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
        name: Option<&'a [u8]>,
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

#[cfg(any(debug_assertions, test))]
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
            EngineObject::Function {
                position,
                num_args,
                name,
            } => {
                write!(
                    f,
                    "<function:{}({})@{}>",
                    name.map_or("<anonymous>", |n| core::str::from_utf8(n)
                        .unwrap_or("<invalid utf-8>")),
                    num_args,
                    position
                )
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

#[cfg(any(debug_assertions, test))]
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

impl<'a, T: ToEngine<'a>> ToEngine<'a> for Result<T, InterpreterError<'a>> {
    fn to_engine(self) -> Result<EngineObject<'a>, InterpreterError<'a>> {
        self.map_err(|e| e)?.to_engine()
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
#[derive(PartialEq, Clone)]
pub enum InterpreterError<'a> {
    /// The name provided was not found in the current variable context.
    InvalidName(&'a [u8]),
    /// A module import failed because the module name was not found in the registered modules.
    ModuleNotResolved(&'a [u8]),
    /// An expression resulted in a value that cannot be used, e.g. trying to use a string literal as a condition.
    InvalidExpressionResult { obj: EngineObject<'a> },
    /// A nonexistent function call was done on a module, or with the wrong number of arguments.
    InvalidModuleFunctionCall { func: &'a [u8], nargs: usize },
    /// A nonexistent member was accessed on a module.
    InvalidModuleMemberAccess { member: &'a [u8] },
    /// Function call on a non-function object.
    InvalidFunctionCall { obj: EngineObject<'a> },
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

#[cfg(debug_assertions)]
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
            other => write!(f, "{:?}", other),
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
            let col = pos - current_line_start - 1;

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

#[derive(PartialEq)]
#[cfg_attr(any(test, debug_assertions), derive(Debug))]
enum BlockScope {
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
    },
}

/// Named variable and value.
struct Variable<'a> {
    name: &'a [u8],
    value: EngineObject<'a>,
}

/// The virtual machine context, holding all state needed for execution.
///
/// The generic parameters specify various limits for the VM, to allow tuning for different environments and use cases.
/// - `STACK_SIZE`: maximum number of variables that can be in scope at once.
/// - `MAX_SCOPE_DEPTH`: maximum depth of nested blocks (e.g. ifs, loops, functions).
/// - `MAX_EXPRESSION_DEPTH`: maximum depth of nested expressions
/// - `MAX_MODULES`: maximum number of modules that can be registered and imported.
pub struct VmContext<
    'a,
    'm,
    const STACK_SIZE: usize = 32,
    const MAX_SCOPE_DEPTH: usize = 32,
    const MAX_EXPRESSION_DEPTH: usize = 16,
    const MAX_MODULES: usize = 4,
> {
    // variables[0..current_block_scope[0]] == global context.
    variables: ArrayVec<Variable<'a>, STACK_SIZE>,

    // We keep track of block/loop/function frames.
    scope_stack: ArrayVec<BlockScope, MAX_SCOPE_DEPTH>,
    current_block_scope: ArrayVec<usize, MAX_SCOPE_DEPTH>,

    modules: ArrayVec<(&'m [u8], &'m mut dyn Module), MAX_MODULES>,

    // Expression evaluation stacks
    expression_stack: ArrayVec<EngineObject<'a>, MAX_EXPRESSION_DEPTH>,
    expression_operator_stack: ArrayVec<(Token<'a>, u8), MAX_EXPRESSION_DEPTH>,

    operations_limit: usize,
    current_operations: usize,

    tokenizer: Tokenizer<'a>,
    // TODO: maybe a "scratch space" for e.g. string concatenation / unescapes, so we don't need to allocate for them
}

impl<
    'a,
    'm,
    const STACK_SIZE: usize,
    const MAX_SCOPE_DEPTH: usize,
    const MAX_EXPRESSION_DEPTH: usize,
    const MAX_MODULES: usize,
> VmContext<'a, 'm, STACK_SIZE, MAX_SCOPE_DEPTH, MAX_EXPRESSION_DEPTH, MAX_MODULES>
{
    const _ASSERT_STACK_SIZE: () = assert!(STACK_SIZE > 0, "STACK_SIZE must be greater than 0");
    const _ASSERT_MAX_SCOPE_DEPTH: () = assert!(
        MAX_SCOPE_DEPTH > 0,
        "MAX_SCOPE_DEPTH must be greater than 0"
    );

    /// Create a new VM context with the given script as input.
    pub fn new(script: &'a [u8]) -> Self {
        let mut vm = Self {
            variables: ArrayVec::new_const(),
            scope_stack: ArrayVec::new_const(),
            current_block_scope: ArrayVec::new_const(),
            tokenizer: Tokenizer::new(script),
            expression_operator_stack: ArrayVec::new_const(),
            expression_stack: ArrayVec::new_const(),
            modules: ArrayVec::new_const(),
            operations_limit: usize::MAX,
            current_operations: 0,
        };
        // global context starts with 0 objects
        unsafe {
            // SAFETY: works since MAX_SCOPE_DEPTH > 0, asserted above
            // We use BlockScope::Normal to represent the global scope
            vm.scope_stack.push_unchecked(BlockScope::Normal);
            vm.current_block_scope.push_unchecked(0);
        }
        vm
    }

    pub fn set_operations_limit(mut self, limit: usize) -> Self {
        self.operations_limit = limit;
        self
    }

    /// Register a module under `name`, available in scripts via `import <name>`.
    pub fn add_module(
        mut self,
        name: &'m [u8],
        m: &'m mut dyn Module,
    ) -> Result<Self, InterpreterError<'a>> {
        self.modules
            .try_push((name, m))
            .map_err(|_| InterpreterError::TooManyModules)?;
        Ok(self)
    }

    pub fn run(&mut self) -> Result<(), InterpreterError<'a>> {
        loop {
            match self.step() {
                Err(e) => return Err(e),
                Ok(false) => break,
                _ => {}
            }
        }
        debug_assert!(
            self.scope_stack.len() == 1,
            "scope stack should be back to global scope at end of execution"
        );
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
    pub fn step(&mut self) -> Result<bool, InterpreterError<'a>> {
        self.check_operations_limit()?;

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
                // Skip both
                self.tokenizer.advance();
                self.tokenizer.advance();

                let value = self.eval_expr()?;
                self.consume_separator()?;

                self.set_var(var_name, value)?;
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
                        name: Some(function_name),
                    },
                )?;
            }
            (Token::If, _) => {
                self.tokenizer.advance();
                let expression_res = self.eval_expr()?.is_true()?;

                self.consume_token(&Token::OpenBrace)?;

                if expression_res {
                    self.enter_scope(BlockScope::If)?;
                } else {
                    // Skip block, expect else or nothing
                    self.skip_block(None)?;

                    // there may be an else block, or nothing
                    let next = self.tokenizer.peek();
                    if next != Token::Else {
                        self.consume_separator()?;
                        return Ok(true);
                    }
                    self.consume_token(&Token::Else)?;
                    self.consume_token(&Token::OpenBrace)?;
                    self.enter_scope(BlockScope::Else)?;
                }

                // Now we are in the correct block
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
                // Empty, separator, or expression
                self.tokenizer.advance();

                let result_expression = match rest {
                    Token::Separator | Token::Eof => EngineObject::Unit,
                    _ => self.eval_expr()?,
                };
                self.consume_separator()?;

                let mut vars_to_remove = 0;
                let return_address = loop {
                    let Some(scope) = self.scope_stack.pop() else {
                        return Err(InterpreterError::ScopeStackEmpty);
                    };
                    vars_to_remove += self
                        .current_block_scope
                        .pop()
                        .ok_or(InterpreterError::ScopeStackEmpty)?;

                    if let BlockScope::Function { return_addr } = scope {
                        // We found the function, now we just need to jump back to the return address and clean up the stack
                        break return_addr;
                    }
                };

                self.variables
                    .truncate(self.variables.len() - vars_to_remove);

                self.expression_stack
                    .try_push(result_expression)
                    .map_err(|_| InterpreterError::ExpressionStackOverflow)?;

                self.tokenizer.set_cursor(return_address);
            }
            (Token::While, _) => {
                self.tokenizer.advance(); // consume 'while'
                let condition_start = self.tokenizer.cursor_pos(); // before condition

                let expression_res = self.eval_expr()?.is_true()?;
                self.consume_token(&Token::OpenBrace)?;
                // cursor is now at body_start

                if expression_res {
                    self.enter_scope(BlockScope::While { condition_start })?;
                    // cursor stays at body_start
                } else {
                    self.skip_block(None)?;
                    // cursor is now after '}'
                }
            }
            (Token::Continue, _) => {
                self.tokenizer.advance();
                let (loop_condition_pos, _) = self.pop_loop(true)?;
                self.evaluate_while_condition(loop_condition_pos)?;
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
                    BlockScope::Function { return_addr } => {
                        // function ends without return statement -> return unit
                        self.expression_stack
                            .try_push(EngineObject::Unit)
                            .map_err(|_| InterpreterError::ExpressionStackOverflow)?;
                        self.tokenizer.set_cursor(return_addr);
                        // Do NOT consume_separator here — the call site handles its own separator.
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
                        // Re-evaluate the condition
                        self.evaluate_while_condition(condition_start)?;
                    }
                    _ => {
                        self.consume_separator()?;
                    }
                }
            }
            (Token::Separator, Token::Eof) | (Token::Eof, _) => return Ok(false),
            // Anything else is just an expression, e.g. a function call
            _ => {
                self.eval_expr()?;
                self.consume_separator()?;
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

    fn enter_scope(&mut self, scope: BlockScope) -> Result<(), InterpreterError<'a>> {
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
    ) -> Result<(), InterpreterError<'a>> {
        let body_end = self.tokenizer.cursor_pos(); // right after '}'
        self.tokenizer.set_cursor(condition_start);
        let continue_loop = self.eval_expr()?.is_true()?;
        self.consume_token(&Token::OpenBrace)?;
        // cursor is now at body_start

        if continue_loop {
            self.enter_scope(BlockScope::While { condition_start })?;
            // cursor stays at body_start
        } else {
            self.tokenizer.set_cursor(body_end);
            self.consume_separator()?;
        }

        Ok(())
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
    pub fn set_var(
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

    pub fn get_var(&mut self, name: &'a [u8]) -> Result<&EngineObject<'a>, InterpreterError<'a>> {
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
                    return Ok(&mut self.variables[i].value);
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
                    return Ok(&mut self.variables[i].value);
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

    /// Evaluate expressions
    fn eval_expr(&mut self) -> Result<EngineObject<'a>, InterpreterError<'a>> {
        let mut expect_operand = true;
        let initial_ops_stack_len = self.expression_operator_stack.len();

        // We use the VmContext expression stack to prevent actual runtime stack overflows.
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
                        // Push sentinel with 0 precedence so nothing pops it until RParen
                        self.expression_operator_stack
                            .try_push((Token::OpenParen, 0))
                            .map_err(|_| InterpreterError::ExpressionStackOverflow)?;

                        // we don't change expect_operand here!
                    }
                    // Unary operators
                    Token::Bang => {
                        self.expression_operator_stack
                            .try_push((Token::Bang, 255)) // highest precedence for unary ops
                            .map_err(|_| InterpreterError::ExpressionStackOverflow)?;
                    }
                    Token::Plus => {
                        // Can be ignored
                    }
                    Token::Minus => {
                        // push a "0-..." onto the stack, to turn unary minus into binary
                        self.expression_stack
                            .try_push(EngineObject::Int(0))
                            .map_err(|_| InterpreterError::ExpressionStackOverflow)?;
                        self.expression_operator_stack
                            .try_push((Token::Minus, 255)) // highest precedence for unary ops
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
                // Expecting operators
                let op = self.tokenizer.peek();
                match op {
                    Token::Equals
                    | Token::Lt
                    | Token::Gt
                    | Token::Lte
                    | Token::Gte
                    | Token::Plus
                    | Token::Minus
                    | Token::Star
                    | Token::Slash => {
                        let (lbp, rbp) = self.infix_binding_power(&op).unwrap();
                        while initial_ops_stack_len < self.expression_operator_stack.len()
                            && let Some((_, top_bp)) = self.expression_operator_stack.last()
                        {
                            if *top_bp >= lbp {
                                self.pop_and_apply()?;
                            } else {
                                break;
                            }
                        }

                        let _ = self.consume_token(&op)?;
                        self.expression_operator_stack
                            .try_push((op, rbp))
                            .map_err(|_| InterpreterError::ExpressionStackOverflow)?;

                        expect_operand = true;
                    }
                    Token::Dot => {
                        let lbp = 100;

                        while initial_ops_stack_len < self.expression_operator_stack.len()
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
                        // Collapse pending Dot operators (e.g. `module.function`) so the callee
                        // on the expression stack is a ModuleMember rather than a bare MemberAccess.
                        while initial_ops_stack_len < self.expression_operator_stack.len() {
                            match self.expression_operator_stack.last() {
                                Some((Token::Dot, _)) => self.pop_and_apply()?,
                                _ => break,
                            }
                        }

                        let Some(function_object) = self.expression_stack.pop() else {
                            return Err(InterpreterError::ExpressionStackEmpty);
                        };

                        self.tokenizer.advance(); // consume '('

                        // Record where args start so we can index them correctly even if
                        // the outer expression already has values on expression_stack.
                        let args_start = self.expression_stack.len();
                        let mut nargs_on_stack = 0;
                        let mut is_first = true;
                        loop {
                            if self.tokenizer.peek() == Token::CloseParen {
                                self.tokenizer.advance();
                                break;
                            }
                            if !is_first {
                                self.consume_token(&Token::Comma)?;
                            }
                            let val = self.eval_expr()?;
                            self.expression_stack
                                .try_push(val)
                                .map_err(|_| InterpreterError::ExpressionStackOverflow)?;
                            nargs_on_stack += 1;
                            is_first = false;
                        }

                        let return_addr = self.tokenizer.cursor_pos();

                        match function_object {
                            EngineObject::Function {
                                position,
                                num_args,
                                name: _,
                            } => {
                                // Push scope only for user-defined functions
                                self.enter_scope(BlockScope::Function { return_addr })?;

                                if num_args != nargs_on_stack {
                                    return Err(InterpreterError::FunctionArgsMismatch {
                                        expected: num_args,
                                        got: nargs_on_stack,
                                        name: None,
                                    });
                                }
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

                                let fn_scope_depth = self.scope_stack.len();
                                loop {
                                    match self.step() {
                                        Ok(true) => {
                                            if self.scope_stack.len() < fn_scope_depth {
                                                break;
                                            }
                                        }
                                        Ok(false) => break,
                                        Err(e) => return Err(e),
                                    }
                                }
                            }
                            EngineObject::ModuleMember { module: idx, name } => {
                                // Call module directly — no scope push needed
                                let result = {
                                    let len = self.expression_stack.len();
                                    let args = &self.expression_stack[len - nargs_on_stack..];
                                    self.modules[idx].1.call(name, args)?
                                };
                                self.expression_stack.truncate(args_start);
                                self.expression_stack
                                    .try_push(result)
                                    .map_err(|_| InterpreterError::ExpressionStackOverflow)?;
                            }
                            obj => {
                                return Err(InterpreterError::InvalidFunctionCall { obj });
                            }
                        }

                        // The function result is now on expression_stack (placed by return or CloseBrace).
                        // Signal that we have an operand so the outer expression can continue (e.g., + 1).
                        expect_operand = false;
                    }
                    Token::CloseParen => {
                        // Only handle this ')' if there is a matching '(' on *our* local op stack.
                        // If there isn't one, this ')' belongs to an outer context (e.g., a
                        // function call's argument list) — just stop evaluating and let the
                        // caller handle it.
                        let has_matching_paren = self.expression_operator_stack
                            [initial_ops_stack_len..]
                            .iter()
                            .any(|(t, _)| *t == Token::OpenParen);

                        if !has_matching_paren {
                            break;
                        }

                        // Execute everything back to the OpenParen
                        while initial_ops_stack_len < self.expression_operator_stack.len()
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

        while initial_ops_stack_len < self.expression_operator_stack.len() {
            self.pop_and_apply()?;
        }

        let res = self
            .expression_stack
            .pop()
            .ok_or(InterpreterError::ExpressionStackEmpty)?;

        self.resolve_if_member(res)
    }

    /// Pops one operator and the necessary number of operands, applies the operator, and pushes the result.
    fn pop_and_apply(&mut self) -> Result<(), InterpreterError<'a>> {
        let (op, _) = self.expression_operator_stack.pop().unwrap();
        let right = self
            .expression_stack
            .pop()
            .ok_or(InterpreterError::ExpressionStackEmpty)?;
        let right = self.resolve_if_member(right)?;

        match op {
            // Unary operators
            Token::Bang => {
                match right {
                    EngineObject::Bool(b) => {
                        self.expression_stack
                            .try_push(EngineObject::Bool(!b))
                            .map_err(|_| InterpreterError::ExpressionStackOverflow)?;
                        return Ok(());
                    }
                    EngineObject::Int(i) => {
                        self.expression_stack
                            .try_push(EngineObject::Int(if i == 0 { 1 } else { 0 }))
                            .map_err(|_| InterpreterError::ExpressionStackOverflow)?;
                        return Ok(());
                    }
                    _ => {
                        return Err(InterpreterError::InvalidUnaryOperation {
                            op,
                            obj: right,
                            program: self.tokenizer.input(),
                            token_pos: self.tokenizer.last_token_pos(),
                        });
                    }
                };
            }
            // Note that minus is handled as 0 - right, so no need to handle it here
            // Also note that plus is allowed, but ignored as unary operator
            _ => {} // Non-unary operators are handled in the main loop
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

            // Comparison operators
            (EngineObject::Int(l), Token::Equals, EngineObject::Int(r)) => {
                left = EngineObject::Bool(*l == *r)
            }
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

            // Compare booleans
            (EngineObject::Bool(l), Token::Equals, EngineObject::Bool(r)) => {
                left = EngineObject::Bool(l == r)
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
            Token::Star | Token::Slash => Some((4, 5)),
            Token::Equals | Token::Lt | Token::Gt | Token::Lte | Token::Gte => Some((1, 2)),
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

#[cfg(test)]
mod tests {
    use super::*;

    use similar_asserts::assert_eq;

    #[test]
    fn test_simple_expr() {
        let mut vm: VmContext<'_, '_> = VmContext::new(b"1 + 2 * 3");
        assert_eq!(vm.eval_expr().unwrap(), 7.to_engine().unwrap());
    }

    #[test]
    fn test_simple_expr2() {
        let mut vm: VmContext<'_, '_> = VmContext::new(b"2 * 3 + 1");
        assert_eq!(vm.eval_expr().unwrap(), 7.to_engine().unwrap());
    }

    #[test]
    fn long_expression() {
        let mut vm: VmContext<'_, '_> = VmContext::new(b"1 + 2 + 3 * 8 + 4 + 5");
        assert_eq!(vm.eval_expr().unwrap(), 36.to_engine().unwrap());
    }

    #[test]
    fn simple_parens_expression() {
        let mut vm: VmContext<'_, '_> = VmContext::new(b"(1 + 2 + 3)");
        assert_eq!(vm.eval_expr().unwrap(), 6.to_engine().unwrap());
    }
    #[test]
    fn parens_expression() {
        let mut vm: VmContext<'_, '_> = VmContext::new(b"(1 + 2 + 3) * (8 + 4 + 5)");
        assert_eq!(vm.eval_expr().unwrap(), 102.to_engine().unwrap());
    }

    #[test]
    fn parens_nested() {
        let mut vm: VmContext<'_, '_> = VmContext::new(b"((1 + 2) * (3 + 4)) * 5");
        assert_eq!(vm.eval_expr().unwrap(), 105.to_engine().unwrap());
    }

    #[test]
    fn parens_nested2() {
        let mut vm: VmContext<'_, '_> = VmContext::new(b"(1 * (2 * (3 * 4))) * 5");
        assert_eq!(vm.eval_expr().unwrap(), 120.to_engine().unwrap());
    }

    #[test]
    fn parens_nested3() {
        let mut vm: VmContext<'_, '_> = VmContext::new(b"5 * (4 * (3 * (2 * 1)))");
        assert_eq!(vm.eval_expr().unwrap(), 120.to_engine().unwrap());
    }

    #[test]
    fn comparison_operators() {
        let mut vm: VmContext<'_, '_> = VmContext::new(b"(1 < 8)");
        assert_eq!(vm.eval_expr().unwrap(), true.to_engine().unwrap());
    }

    #[test]
    fn unary_operators() {
        let mut vm: VmContext<'_, '_> = VmContext::new(b"-5");
        assert_eq!(vm.eval_expr().unwrap(), (-5).to_engine().unwrap());
    }
    #[test]
    fn unary_operators2() {
        let mut vm: VmContext<'_, '_> = VmContext::new(b"!5");
        assert_eq!(vm.eval_expr().unwrap(), 0.to_engine().unwrap());
    }
    #[test]
    fn unary_operators3() {
        let mut vm: VmContext<'_, '_> = VmContext::new(b"!0");
        assert_eq!(vm.eval_expr().unwrap(), 1.to_engine().unwrap());
    }

    #[test]
    fn assign_variables() {
        let mut vm: VmContext<'_, '_> = VmContext::new(b"a = 5 + 5;");
        assert!(vm.run().is_ok());
        assert_eq!(*vm.get_var(b"a").unwrap(), 10.to_engine().unwrap());
    }

    #[test]
    fn assign_multiple_variables() {
        let mut vm: VmContext<'_, '_> = VmContext::new(b"a = 5 + 5; b = a + 5;");
        assert!(vm.run().is_ok());
        assert_eq!(*vm.get_var(b"a").unwrap(), 10.to_engine().unwrap());
        assert_eq!(*vm.get_var(b"b").unwrap(), 15.to_engine().unwrap());
    }

    #[test]
    fn assign_too_many_variables() {
        // Limit stack to at most 2 variables
        let mut vm: VmContext<'_, '_, 2, 16, 8, 16> =
            VmContext::new(b"a = 5 + 5; b = a + 5; c = b + a;");
        assert!(matches!(
            vm.run(),
            Err(InterpreterError::VariableStackOverflow)
        ));
    }

    #[test]
    fn declare_function() {
        let mut vm: VmContext<'_, '_> = VmContext::new(
            br#"fn test(a, b) { return a + b }
            fn test2() { return 7 }
        "#,
        );
        vm.run().expect("Running VM to declare function");
        {
            let test_func = vm.get_var(b"test").expect("function to be variable");
            assert!(matches!(
                test_func,
                EngineObject::Function {
                    position: 8,
                    num_args: 2,
                    name: Some(b"test")
                }
            ));
        }

        let test_func2 = vm.get_var(b"test2").expect("function to be variable");
        assert!(matches!(
            test_func2,
            EngineObject::Function {
                position: 52,
                num_args: 0,
                name: Some(b"test2")
            }
        ));
    }

    #[test]
    fn if_else() {
        let mut vm: VmContext<'_, '_> = VmContext::new(
            br#"a = 5;
            b = 0;
            if a > 3 {
                b = 10;
            } else {
                b = 20;
            }
        "#,
        );
        vm.run().expect("Running VM with if-else");
        assert_eq!(*vm.get_var(b"b").unwrap(), 10.to_engine().unwrap());
    }

    #[test]
    fn if_only() {
        let mut vm: VmContext<'_, '_> = VmContext::new(
            br#"a = 5;
            b = 0;
            if a > 3 {
                b = 10;
            }
        "#,
        );
        vm.run().expect("Running VM with if-else");
        assert_eq!(*vm.get_var(b"b").unwrap(), 10.to_engine().unwrap());
    }

    #[test]
    fn if_nested() {
        let mut vm: VmContext<'_, '_> = VmContext::new(
            br#"a = 5;
            b = 0;
            if a > 3 {
                if (a < 10) {
                    b = 10;
                }
            }
        "#,
        );
        vm.run().expect("Running VM with if-else");
        assert_eq!(*vm.get_var(b"b").unwrap(), 10.to_engine().unwrap());
    }

    #[test]
    fn function_call() {
        let mut vm: VmContext<'_, '_> = VmContext::new(
            br#"fn val() { return 5; }
                c = val();
            "#,
        );
        vm.run().expect("Running VM with function call");
        assert_eq!(*vm.get_var(b"c").unwrap(), 5.to_engine().unwrap());
    }

    #[test]
    fn stack_overflow() {
        let mut vm: VmContext<'_, '_> = VmContext::new(
            br#"fn recurse() { return recurse(); }
                recurse();
            "#,
        );
        assert!(matches!(
            vm.run(),
            Err(InterpreterError::ScopeStackExhausted)
        ));
    }

    #[test]
    fn fibonacci() {
        let mut vm: VmContext<'_, '_> = VmContext::new(
            br#"fn fib(n) {
                if n <= 1 {
                    return n;
                } else {
                    return fib(n - 1) + fib(n - 2);
                }
            }
            result = fib(10);
            "#,
        );
        vm.run().expect("Running VM with Fibonacci function");
        assert_eq!(*vm.get_var(b"result").unwrap(), 55.to_engine().unwrap());
    }

    #[test]
    fn function_call_two_args() {
        let mut vm: VmContext<'_, '_> =
            VmContext::new(b"fn add(a, b) { return a + b; }\nresult = add(2, 3);");
        vm.run().unwrap();
        assert_eq!(*vm.get_var(b"result").unwrap(), 5.to_engine().unwrap());
    }

    #[test]
    fn function_call_implicit_unit() {
        let mut vm: VmContext<'_, '_> = VmContext::new(b"fn noop() {}\nresult = noop();");
        vm.run().unwrap();
        assert_eq!(*vm.get_var(b"result").unwrap(), EngineObject::Unit);
    }

    #[test]
    fn function_call_in_expression() {
        let mut vm: VmContext<'_, '_> =
            VmContext::new(b"fn add(a, b) { return a + b; }\nresult = add(2, 3) + 1;");
        vm.run().unwrap();
        assert_eq!(*vm.get_var(b"result").unwrap(), 6.to_engine().unwrap());
    }

    #[test]
    fn nested_function_call_as_arg() {
        let mut vm: VmContext<'_, '_> = VmContext::new(b"fn double(x) { return x * 2; }\nfn add(a, b) { return a + b; }\nresult = add(double(2), 3);");
        vm.run().unwrap();
        assert_eq!(*vm.get_var(b"result").unwrap(), 7.to_engine().unwrap());
    }

    #[test]
    fn while_loop_basic() {
        let mut vm: VmContext<'_, '_> = VmContext::new(b"i = 0;\nwhile i < 3 {\ni = i + 1;\n}");
        vm.run().unwrap();
        assert_eq!(*vm.get_var(b"i").unwrap(), 3.to_engine().unwrap());
    }

    #[test]
    fn while_loop_never_executes() {
        let mut vm: VmContext<'_, '_> = VmContext::new(b"i = 5;\nwhile i < 3 {\ni = i + 1;\n}");
        vm.run().unwrap();
        assert_eq!(*vm.get_var(b"i").unwrap(), 5.to_engine().unwrap());
    }

    #[test]
    fn while_loop_accumulates() {
        let mut vm: VmContext<'_, '_> =
            VmContext::new(b"i = 0;\nsum = 0;\nwhile i < 5 {\nsum = sum + i;\ni = i + 1;\n}");
        vm.run().unwrap();
        assert_eq!(*vm.get_var(b"sum").unwrap(), 10.to_engine().unwrap());
    }

    #[test]
    fn while_loop_continue() {
        let mut vm: VmContext<'_, '_> = VmContext::new(
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
        );
        vm.run().unwrap();
        assert_eq!(*vm.get_var(b"sum").unwrap(), 4.to_engine().unwrap());
        assert_eq!(*vm.get_var(b"i").unwrap(), 4.to_engine().unwrap());
    }

    #[test]
    fn while_loop_break() {
        let mut vm: VmContext<'_, '_> = VmContext::new(
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
        );
        vm.run().unwrap();
        assert_eq!(*vm.get_var(b"sum").unwrap(), 1.to_engine().unwrap());
        assert_eq!(*vm.get_var(b"i").unwrap(), 2.to_engine().unwrap());
    }

    #[test]
    fn while_loop_nested() {
        let mut vm: VmContext<'_, '_> = VmContext::new(
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
        );
        vm.run().unwrap();
        assert_eq!(*vm.get_var(b"sum").unwrap(), 9.to_engine().unwrap());
    }

    #[test]
    fn continue_outside_loop() {
        let mut vm: VmContext<'_, '_> = VmContext::new(b"continue;");
        assert!(matches!(
            vm.run(),
            Err(InterpreterError::ContinueOutsideLoop)
        ));
    }

    #[test]
    fn break_outside_loop() {
        let mut vm: VmContext<'_, '_> = VmContext::new(b"if true { break; }");
        assert!(matches!(vm.run(), Err(InterpreterError::BreakOutsideLoop)));
    }

    use nova_macros::{engine_module, script_module};

    #[engine_module]
    struct MathModule {}

    #[script_module]
    impl MathModule {
        pub fn add(&self, a: i32, b: i32) -> i32 {
            a + b
        }
    }

    #[test]
    fn math_module() {
        let mut math = MathModule {};
        let mut vm: VmContext<'_, '_> = VmContext::new(b"import math; i = math.add(1, 2);")
            .add_module(b"math", &mut math)
            .unwrap();
        vm.run().unwrap();
        assert_eq!(*vm.get_var(b"i").unwrap(), 3.to_engine().unwrap());
    }

    #[engine_module]
    struct FancyMathModule {
        MAX_INT: i32,
    }

    #[script_module]
    impl FancyMathModule {
        fn set_max(&mut self, max: i32) {
            self.MAX_INT = max;
        }
    }

    #[test]
    fn math_module_fancy() {
        let mut math = FancyMathModule { MAX_INT: 100 };
        let mut vm: VmContext<'_, '_> =
            VmContext::new(b"import fancy_math; i = fancy_math.MAX_INT;")
                .add_module(b"fancy_math", &mut math)
                .unwrap();
        vm.run().unwrap();
        assert_eq!(*vm.get_var(b"i").unwrap(), 100.to_engine().unwrap());
    }

    #[test]
    fn invalid_function_access() {
        let mut math = MathModule {};
        let mut vm: VmContext<'_, '_> = VmContext::new(b"import math; i = math.subtract(1, 2);")
            .add_module(b"math", &mut math)
            .unwrap();
        assert!(matches!(
            vm.run(),
            Err(InterpreterError::InvalidModuleFunctionCall {
                func: b"subtract",
                nargs: 2,
            })
        ));
    }
    #[test]
    fn invalid_member_access() {
        let mut math = MathModule {};
        let mut vm: VmContext<'_, '_> = VmContext::new(b"import math; i = math.MAX;")
            .add_module(b"math", &mut math)
            .unwrap();
        assert!(matches!(
            vm.run(),
            Err(InterpreterError::InvalidModuleMemberAccess { member: b"MAX" })
        ));
    }

    #[test]
    fn dont_set() {
        let mut math = FancyMathModule { MAX_INT: 100 };
        let mut vm: VmContext<'_, '_> =
            VmContext::new(b"import fancy_math; fancy_math.MAX_INT = 200;")
                .add_module(b"fancy_math", &mut math)
                .unwrap();
        assert!(matches!(vm.run(), Err(_)));
    }

    #[test]
    fn fibonacci_two() {
        let mut vm: VmContext<'_, '_> = VmContext::new(
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
        );
        vm.run().expect("Running VM with Fibonacci function");
        assert_eq!(*vm.get_var(b"it_res").unwrap(), 55.to_engine().unwrap());
        assert_eq!(*vm.get_var(b"rc_res").unwrap(), 55.to_engine().unwrap());
        assert_eq!(*vm.get_var(b"same").unwrap(), true.to_engine().unwrap());
    }

    #[test]
    fn mul_overflow() {
        let mut vm: VmContext<'_, '_> = VmContext::new(
            br#"
            n = 25;
            while true {
                n = n * 25;
            }
            "#,
        );
        assert!(matches!(
            vm.run(),
            Err(InterpreterError::OperatorOverflow {
                op: Token::Star,
                ..
            })
        ));
    }

    #[test]
    fn test_sibling() {
        let mut vm: VmContext<'_, '_> = VmContext::new(
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
        );
        vm.run().expect("Running VM with sibling functions");
        assert_eq!(*vm.get_var(b"result").unwrap(), true.to_engine().unwrap());
    }

    #[test]
    fn test_collatz() {
        let mut vm: VmContext<'_, '_> = VmContext::new(
            br#"
            fn computeChainLength(n) {
                steps = 0;
                while n > 1 {
                    if n == 1 {
                        break;
                    }
                    if n * 2 == n {
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
        );
        vm.run().expect("Running VM with Collatz function");
        assert_eq!(
            *vm.get_var(b"max_number").unwrap(),
            871.to_engine().unwrap()
        );
    }
}
