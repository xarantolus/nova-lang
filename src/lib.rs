#![cfg_attr(not(test), no_std)]

use core::cell::RefCell;

#[cfg(any(debug_assertions, test))]
use core::fmt::Debug;

use arrayvec::ArrayVec;

use crate::tokenizer::{Token, Tokenizer};

mod tokenizer;

pub trait Module<'a> {
    fn call(
        &mut self,
        func: &'a [u8],
        args: &[EngineObject<'a>],
    ) -> Result<EngineObject<'a>, InterpreterError<'a>>;
}

pub type ModuleResolver<'a> = fn(&'a [u8]) -> Option<&'a mut dyn Module<'a>>;

#[derive(Clone)]
pub enum EngineObject<'a> {
    Module(&'a RefCell<dyn Module<'a>>),
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
        module: &'a RefCell<dyn Module<'a>>,
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

impl<'a> Into<EngineObject<'a>> for i32 {
    fn into(self) -> EngineObject<'a> {
        EngineObject::Int(self)
    }
}

impl<'a> TryInto<i32> for EngineObject<'a> {
    type Error = InterpreterError<'a>;

    fn try_into(self) -> Result<i32, Self::Error> {
        match self {
            EngineObject::Int(i) => Ok(i),
            _ => Err(InterpreterError::InvalidConversion {
                from: self,
                to: "i32",
            }),
        }
    }
}

impl<'a> Into<EngineObject<'a>> for bool {
    fn into(self) -> EngineObject<'a> {
        EngineObject::Bool(self)
    }
}

impl<'a> TryInto<bool> for EngineObject<'a> {
    type Error = InterpreterError<'a>;

    fn try_into(self) -> Result<bool, Self::Error> {
        match self {
            EngineObject::Bool(b) => Ok(b),
            _ => Err(InterpreterError::InvalidConversion {
                from: self,
                to: "bool",
            }),
        }
    }
}
#[derive(PartialEq, Clone)]
#[cfg_attr(debug_assertions, derive(Debug))]
pub enum InterpreterError<'a> {
    NameError(&'a [u8]),
    InvalidExpressionResult {
        obj: EngineObject<'a>,
    },
    InvalidUnaryOperation {
        op: Token<'a>,
        obj: EngineObject<'a>,
    },
    InvalidOperation {
        op: Token<'a>,
        left: EngineObject<'a>,
        right: EngineObject<'a>,
    },
    InvalidConversion {
        from: EngineObject<'a>,
        to: &'static str,
    },
    InvalidExpression(Token<'a>),
    UnexpectedToken {
        expected: Token<'a>,
        found: Token<'a>,
    },
    ExpectedCallable {
        got: EngineObject<'a>,
    },
    FunctionArgsMismatch {
        expected: usize,
        got: usize,
        name: Option<&'a [u8]>,
    },
    ScopeVariableMismatch,
    ScopeStackEmpty,
    ScopeStackExhausted,
    ExpressionStackEmpty,
    ExpressionStackOverflow,
    TooManySteps,
    ObjectStackOverflow,
    UnexpectedEoF,
}

#[derive(PartialEq)]
#[cfg_attr(any(test, debug_assertions), derive(Debug))]
enum BlockScope {
    Normal,
    While {
        // position is the position of the while expression
        position: usize,
        // position after the end brace, if known
        end_position: Option<usize>,
    },
    If,
    Else,
    Function {
        // We act as the frame pointer here
        return_addr: usize,
    },
}

struct Variable<'a> {
    name: &'a [u8],
    value: EngineObject<'a>,
}

pub struct VmContext<
    'a,
    const STACK_SIZE: usize = 32,
    const MAX_CALL_DEPTH: usize = 16,
    const MAX_SCOPE_DEPTH: usize = 8,
    const MAX_EXPRESSION_DEPTH: usize = 16,
> {
    // locals[0..current_function_objects[0]] == global context.
    stack: ArrayVec<Variable<'a>, STACK_SIZE>,

    // We keep track of block/loop/function frames.
    scope_stack: ArrayVec<BlockScope, MAX_SCOPE_DEPTH>,
    current_block_scope: ArrayVec<usize, MAX_SCOPE_DEPTH>,

    module_resolver: Option<ModuleResolver<'a>>,

    // Expression evaluation stacks
    expression_stack: ArrayVec<EngineObject<'a>, MAX_EXPRESSION_DEPTH>,
    expression_operator_stack: ArrayVec<(Token<'a>, u8), MAX_EXPRESSION_DEPTH>,

    tokenizer: Tokenizer<'a>,
    // TODO: maybe a "scratch space" for e.g. string concatenation / unescapes, so we don't need to allocate for them
}

impl<'a, const STACK_SIZE: usize, const MAX_CALL_DEPTH: usize, const MAX_SCOPE_DEPTH: usize>
    VmContext<'a, STACK_SIZE, MAX_CALL_DEPTH, MAX_SCOPE_DEPTH>
{
    const _ASSERT_STACK_SIZE: () = assert!(STACK_SIZE > 0, "STACK_SIZE must be greater than 0");
    const _ASSERT_MAX_CALL_DEPTH: () =
        assert!(MAX_CALL_DEPTH > 0, "MAX_CALL_DEPTH must be greater than 0");
    const _ASSERT_MAX_SCOPE_DEPTH: () = assert!(
        MAX_SCOPE_DEPTH > 0,
        "MAX_SCOPE_DEPTH must be greater than 0"
    );

    pub fn new(script: &'a [u8]) -> Self {
        Self::new_with_modules(script, None)
    }

    pub fn new_with_modules(script: &'a [u8], module_resolver: Option<ModuleResolver<'a>>) -> Self {
        let mut vm = Self {
            stack: ArrayVec::new_const(),
            scope_stack: ArrayVec::new_const(),
            current_block_scope: ArrayVec::new_const(),
            tokenizer: Tokenizer::new(script),
            expression_operator_stack: ArrayVec::new_const(),
            expression_stack: ArrayVec::new_const(),
            module_resolver,
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

    pub fn run(&mut self) -> Result<(), InterpreterError<'a>> {
        loop {
            match self.step() {
                Err(e) => return Err(e),
                Ok(false) => break,
                _ => {}
            }
        }
        Ok(())
    }

    pub fn run_bounded(&mut self, max_steps: usize) -> Result<(), InterpreterError<'a>> {
        let mut i = 0;
        loop {
            if i > max_steps {
                return Err(InterpreterError::TooManySteps);
            }
            match self.step() {
                Err(e) => return Err(e),
                Ok(false) => break,
                _ => {}
            }
            i += 1;
        }
        Ok(())
    }

    // Returns: Ok(true) if work was done, Ok(false) if EOF, Err on error
    pub fn step(&mut self) -> Result<bool, InterpreterError<'a>> {
        let (first_token, second_token) = self.tokenizer.peek2();

        match (first_token, second_token) {
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
                                found: tok,
                            });
                        }
                    }
                }

                self.consume_token(&Token::OpenBrace)?;

                self.skip_block()?;

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
                    self.scope_stack
                        .try_push(BlockScope::If)
                        .map_err(|_| InterpreterError::ScopeStackExhausted)?;
                    self.current_block_scope
                        .try_push(0)
                        .map_err(|_| InterpreterError::ScopeStackExhausted)?;
                } else {
                    // Skip block, expect else
                    self.skip_block()?;
                    self.consume_token(&Token::Else)?;
                    self.consume_token(&Token::OpenBrace)?;
                    self.scope_stack
                        .try_push(BlockScope::Else)
                        .map_err(|_| InterpreterError::ScopeStackExhausted)?;
                    self.current_block_scope
                        .try_push(0)
                        .map_err(|_| InterpreterError::ScopeStackExhausted)?;
                }

                // Now we are in the correct block
            }
            (Token::Else, _) => {
                // If we hit this, we have an else without a matching if...
                return Err(InterpreterError::UnexpectedToken {
                    expected: Token::If,
                    found: Token::Else,
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

                self.stack.truncate(self.stack.len() - vars_to_remove);

                self.expression_stack
                    .try_push(result_expression)
                    .map_err(|_| InterpreterError::ObjectStackOverflow)?;

                self.tokenizer.set_cursor(return_address);
            }
            (Token::While, _) => {
                // while + condition
                unimplemented!("while")
            }
            (Token::OpenBrace, _) => {}
            (Token::CloseBrace, _) => {
                // Blocks

                // Consume the brace
                self.tokenizer.advance();

                // pop one off the scope stack?
                let Some(block) = self.scope_stack.pop() else {
                    return Err(InterpreterError::ScopeStackEmpty);
                };
                // TODO: if we pop a function block, we have a function end without return?
                // Either return unit or error

                // The count for the block we just popped
                let var_count = self
                    .current_block_scope
                    .pop()
                    .ok_or(InterpreterError::ScopeStackEmpty)?;

                let next = self.tokenizer.peek();
                if block == BlockScope::If && next == Token::Else {
                    // We executed the if, so we skip the else block
                    self.tokenizer.advance();
                    self.consume_token(&Token::OpenBrace)?;
                    self.skip_block()?;
                }

                // Delete scope-specific variables
                self.stack.truncate(self.stack.len() - var_count);

                self.consume_separator()?;
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

    /// Consumes next tokens, ensuring it is the expected one, otherwise returns an error.
    fn consume_token(&mut self, expected: &Token<'a>) -> Result<(), InterpreterError<'a>> {
        let token = self.tokenizer.advance();
        if token == *expected {
            Ok(())
        } else {
            Err(InterpreterError::UnexpectedToken {
                expected: *expected,
                found: token,
            })
        }
    }

    /// Consumes separator tokens (optional!)
    fn consume_separator(&mut self) -> Result<(), InterpreterError<'a>> {
        let token = self.tokenizer.advance();
        if Token::Separator == token || Token::Eof == token {
            Ok(())
        } else {
            Err(InterpreterError::UnexpectedToken {
                expected: Token::Separator,
                found: token,
            })
        }
    }

    pub fn set_var(
        &mut self,
        name: &'a [u8],
        value: EngineObject<'a>,
    ) -> Result<(), InterpreterError<'a>> {
        if self.stack.len() >= STACK_SIZE {
            return Err(InterpreterError::ObjectStackOverflow);
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

        let stack_len = self.stack.len();
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

        // 3. Perform the search and update (Borrow Checker Safe: we are iterating indices, not references to self)
        for i in locals_range.chain(globals_range) {
            if self.stack[i].name == name {
                self.stack[i].value = value;
                return Ok(());
            }
        }

        // 4. Not found: Insert new variable into current scope
        unsafe {
            // SAFETY: checked at function start
            self.stack.push_unchecked(Variable { name, value });
        };

        // Update the count for the current specific block (top of the scope stack)
        if let Some(last) = self.current_block_scope.last_mut() {
            *last += 1;
        }

        Ok(())
    }

    pub fn get_var(
        &mut self,
        name: &'a [u8],
    ) -> Result<&mut EngineObject<'a>, InterpreterError<'a>> {
        let mut current_stack_index = self.stack.len();

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
                if self.stack[i].name == name {
                    return Ok(&mut self.stack[i].value);
                }
            }

            if let BlockScope::Function { .. } = scope {
                break;
            }
        }

        // Fallback to global context
        if let Some(&global_count) = self.current_block_scope.first() {
            for i in (0..global_count).rev() {
                if self.stack[i].name == name {
                    return Ok(&mut self.stack[i].value);
                }
            }
        }

        Err(InterpreterError::NameError(name))
    }

    fn eval_expr(&mut self) -> Result<EngineObject<'a>, InterpreterError<'a>> {
        let mut expect_operand = true;
        let initial_ops_stack_len = self.expression_operator_stack.len();

        // We use the VmContext expression stack to prevent actual runtime stack overflows.
        loop {
            if expect_operand {
                match self.tokenizer.advance() {
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
                        return Err(InterpreterError::InvalidExpression(token));
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
                                // We push the name as a StringLiteral so `pop_and_apply` can use it
                                self.expression_stack
                                    .try_push(EngineObject::MemberAccess { name: id })
                                    .map_err(|_| InterpreterError::ExpressionStackOverflow)?;
                            }
                            t => {
                                return Err(InterpreterError::UnexpectedToken {
                                    expected: Token::Identifier(&[]),
                                    found: t,
                                });
                            }
                        }
                        expect_operand = false;
                    }
                    Token::OpenParen => {
                        // We land here on function calls
                        // TODO: figure out module function calls with dot operator
                        self.tokenizer.advance();

                        // First evaluate expressions passed to function
                        // They are pushed onto the stack, and will need to be bound in reverse order
                        let mut nargs = 0;
                        let mut is_first = true;
                        loop {
                            let next_token = self.tokenizer.peek();
                            if next_token == Token::CloseParen {
                                self.tokenizer.advance();
                                break;
                            }

                            if !is_first {
                                self.consume_token(&Token::Comma)?;
                            }

                            let expr_res = self.eval_expr()?;
                            self.expression_stack
                                .try_push(expr_res)
                                .map_err(|_| InterpreterError::ExpressionStackOverflow)?;
                            nargs += 1;
                            is_first = false;
                        }

                        match self.expression_stack.pop() {
                            Some(EngineObject::Function {
                                position,
                                num_args,
                                name,
                            }) => {
                                if num_args != nargs {
                                    return Err(InterpreterError::FunctionArgsMismatch {
                                        expected: num_args,
                                        got: nargs,
                                        name,
                                    });
                                }

                                // push instruction pointer
                                let current_pos = self.tokenizer.cursor_pos();
                                self.scope_stack
                                    .try_push(BlockScope::Function {
                                        return_addr: current_pos,
                                    })
                                    .map_err(|_| InterpreterError::ScopeStackExhausted)?;
                                self.current_block_scope
                                    .try_push(num_args)
                                    .map_err(|_| InterpreterError::ScopeStackExhausted)?;

                                // jump to function
                                self.tokenizer.set_cursor(position);
                                // We should now be directly after the opening parentheses of the arguments
                                // so we are after "fn foo("
                                for i in 0..num_args {
                                    //get name of argument from function definition, then bind values
                                    let arg_token = self.tokenizer.advance();
                                    let arg_name = match arg_token {
                                        Token::Identifier(name) => name,
                                        t => {
                                            return Err(InterpreterError::UnexpectedToken {
                                                expected: Token::Identifier(&[]),
                                                found: t,
                                            });
                                        }
                                    };
                                    let value = self.expression_stack
                                        [self.expression_stack.len() - num_args + i]
                                        .clone();
                                    self.set_var(arg_name, value)?;
                                    if i < num_args - 1 {
                                        self.consume_token(&Token::Comma)?;
                                    }
                                }
                                // Remove args from stack
                                self.expression_stack
                                    .truncate(self.expression_stack.len() - num_args);

                                self.consume_token(&Token::CloseParen)?;
                                self.consume_token(&Token::OpenBrace)?;
                            }
                            // TODO: figure out module function calls?
                            _ => {
                                return Err(InterpreterError::ExpectedCallable {
                                    got: self
                                        .expression_stack
                                        .last()
                                        .unwrap_or(&EngineObject::Unit)
                                        .clone(),
                                });
                            }
                        };
                        expect_operand = true;
                    }
                    Token::CloseParen => {
                        // Execute everything back to the OpenParen
                        while initial_ops_stack_len < self.expression_operator_stack.len()
                            && let Some((top_op, _)) = self.expression_operator_stack.last()
                        {
                            if *top_op == Token::OpenParen {
                                break;
                            }
                            self.pop_and_apply()?;
                        }

                        if self.expression_operator_stack.pop().map(|(t, _)| t)
                            != Some(Token::OpenParen)
                        {
                            return Err(InterpreterError::InvalidExpression(Token::CloseParen));
                        }
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

        self.expression_stack
            .pop()
            .ok_or(InterpreterError::ExpressionStackEmpty)
    }

    fn pop_and_apply(&mut self) -> Result<(), InterpreterError<'a>> {
        let (op, _) = self.expression_operator_stack.pop().unwrap();
        let right = self
            .expression_stack
            .pop()
            .ok_or(InterpreterError::ExpressionStackEmpty)?;

        match op {
            // Unary operators
            Token::Bang => {
                if let EngineObject::Bool(b) = right {
                    self.expression_stack
                        .try_push(EngineObject::Bool(!b))
                        .map_err(|_| InterpreterError::ExpressionStackOverflow)?;
                    return Ok(());
                } else if let EngineObject::Int(i) = right {
                    self.expression_stack
                        .try_push(EngineObject::Int(if i == 0 { 1 } else { 0 }))
                        .map_err(|_| InterpreterError::ExpressionStackOverflow)?;
                    return Ok(());
                } else {
                    return Err(InterpreterError::InvalidUnaryOperation { op, obj: right });
                }
            }
            // Note that minus is handled as 0 - right, so no need to handle it here
            // Also note that plus is allowed, but ignored as unary operator
            _ => {} // Non-unary operators are handled in the main loop
        }

        let mut left = self
            .expression_stack
            .pop()
            .ok_or(InterpreterError::ExpressionStackEmpty)?;

        match (&mut left, op, &right) {
            // Integer math
            (EngineObject::Int(l), Token::Plus, EngineObject::Int(r)) => *l += r,
            (EngineObject::Int(l), Token::Minus, EngineObject::Int(r)) => *l -= r,
            (EngineObject::Int(l), Token::Star, EngineObject::Int(r)) => *l *= r,
            (EngineObject::Int(l), Token::Slash, EngineObject::Int(r)) => *l /= r,

            // Comparison operators
            (EngineObject::Int(l), Token::Equals, EngineObject::Int(r)) => {
                left = EngineObject::Bool(l == r)
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
            (EngineObject::Module(m), Token::Dot, EngineObject::MemberAccess { name }) => {
                // Your member access logic here...
                // e.g. *left = m.borrow().get_member(content)?;
                // TODO: something like m.borrow_mut().call(name, args), but need to resolve args
                unimplemented!(
                    "Resolve member {} on module",
                    core::str::from_utf8(name).unwrap()
                );
            }

            // Error
            _ => {
                return Err(InterpreterError::InvalidOperation {
                    op,
                    left: left.clone(),
                    right: right.clone(),
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

    fn skip_block(&mut self) -> Result<(), InterpreterError<'a>> {
        let mut depth = 1; // We assume we just passed the opening '{' (or are about to)

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
        let mut vm: VmContext<'_> = VmContext::new(b"1 + 2 * 3");
        assert_eq!(vm.eval_expr().unwrap(), 7.into());
    }

    #[test]
    fn test_simple_expr2() {
        let mut vm: VmContext<'_> = VmContext::new(b"2 * 3 + 1");
        assert_eq!(vm.eval_expr().unwrap(), 7.into());
    }

    #[test]
    fn long_expression() {
        let mut vm: VmContext<'_> = VmContext::new(b"1 + 2 + 3 * 8 + 4 + 5");
        assert_eq!(vm.eval_expr().unwrap(), 36.into());
    }

    #[test]
    fn simple_parens_expression() {
        let mut vm: VmContext<'_> = VmContext::new(b"(1 + 2 + 3)");
        assert_eq!(vm.eval_expr().unwrap(), 6.into());
    }
    #[test]
    fn parens_expression() {
        let mut vm: VmContext<'_> = VmContext::new(b"(1 + 2 + 3) * (8 + 4 + 5)");
        assert_eq!(vm.eval_expr().unwrap(), 102.into());
    }

    #[test]
    fn parens_nested() {
        let mut vm: VmContext<'_> = VmContext::new(b"((1 + 2) * (3 + 4)) * 5");
        assert_eq!(vm.eval_expr().unwrap(), 105.into());
    }

    #[test]
    fn parens_nested2() {
        let mut vm: VmContext<'_> = VmContext::new(b"(1 * (2 * (3 * 4))) * 5");
        assert_eq!(vm.eval_expr().unwrap(), 120.into());
    }

    #[test]
    fn parens_nested3() {
        let mut vm: VmContext<'_> = VmContext::new(b"5 * (4 * (3 * (2 * 1)))");
        assert_eq!(vm.eval_expr().unwrap(), 120.into());
    }

    #[test]
    fn comparison_operators() {
        let mut vm: VmContext<'_> = VmContext::new(b"(1 < 8)");
        assert_eq!(vm.eval_expr().unwrap(), true.into());
    }

    #[test]
    fn unary_operators() {
        let mut vm: VmContext<'_> = VmContext::new(b"-5");
        assert_eq!(vm.eval_expr().unwrap(), (-5).into());
    }
    #[test]
    fn unary_operators2() {
        let mut vm: VmContext<'_> = VmContext::new(b"!5");
        assert_eq!(vm.eval_expr().unwrap(), 0.into());
    }
    #[test]
    fn unary_operators3() {
        let mut vm: VmContext<'_> = VmContext::new(b"!0");
        assert_eq!(vm.eval_expr().unwrap(), 1.into());
    }

    #[test]
    fn assign_variables() {
        let mut vm: VmContext<'_> = VmContext::new(b"a = 5 + 5;");
        assert!(vm.run().is_ok());
        assert_eq!(*vm.get_var(b"a").unwrap(), 10.into());
    }

    #[test]
    fn assign_multiple_variables() {
        let mut vm: VmContext<'_> = VmContext::new(b"a = 5 + 5; b = a + 5;");
        assert!(vm.run().is_ok());
        assert_eq!(*vm.get_var(b"a").unwrap(), 10.into());
        assert_eq!(*vm.get_var(b"b").unwrap(), 15.into());
    }

    #[test]
    fn assign_too_many_variables() {
        // Limit stack to at most 2 variables
        let mut vm: VmContext<'_, 2, 16, 8, 16> =
            VmContext::new(b"a = 5 + 5; b = a + 5; c = b + a;");
        assert!(matches!(
            vm.run(),
            Err(InterpreterError::ObjectStackOverflow)
        ));
    }

    #[test]
    fn declare_function() {
        let mut vm: VmContext<'_> = VmContext::new(
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
        let mut vm: VmContext<'_> = VmContext::new(
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
        assert_eq!(*vm.get_var(b"b").unwrap(), 10.into());
    }

    #[test]
    fn if_only() {
        let mut vm: VmContext<'_> = VmContext::new(
            br#"a = 5;
            b = 0;
            if a > 3 {
                b = 10;
            }
        "#,
        );
        vm.run().expect("Running VM with if-else");
        assert_eq!(*vm.get_var(b"b").unwrap(), 10.into());
    }

    #[test]
    fn if_nested() {
        let mut vm: VmContext<'_> = VmContext::new(
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
        assert_eq!(*vm.get_var(b"b").unwrap(), 10.into());
    }

    #[test]
    fn function_call() {
        let mut vm: VmContext<'_> = VmContext::new(
            br#"fn val() { return 5; }
                c = val();
            "#,
        );
        vm.run().expect("Running VM with function call");
        assert_eq!(*vm.get_var(b"c").unwrap(), 5.into());
    }
}
