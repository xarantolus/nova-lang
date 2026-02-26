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

#[derive(PartialEq, Clone)]
#[cfg_attr(debug_assertions, derive(Debug))]
pub enum InterpreterError<'a> {
    NameError(&'a [u8]),
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
    ExpressionStackExhausted,
    TooManySteps,
    StackOverflow,
    UnexpectedEoF,
}

#[derive(Clone, Copy, Default)]
struct LoopFrame {
    start_cursor: usize,
    // optimization for loops that have at least one iteration
    // if we end the loop and already know the end cursor, we can jump there directly instead of having to scan the block
    end_cursor: Option<usize>,
}

struct Variable<'a> {
    name: &'a [u8],
    value: EngineObject<'a>,
}

pub struct VmContext<
    'a,
    const STACK_SIZE: usize = 32,
    const MAX_CALL_DEPTH: usize = 16,
    const MAX_LOOP_DEPTH: usize = 8,
    const MAX_EXPRESSION_DEPTH: usize = 16,
> {
    // locals[0..current_function_objects[0]] == global context.
    stack: ArrayVec<Variable<'a>, STACK_SIZE>,

    // We keep track of frame pointers to know where to return to after a function call.
    frame_pointers: ArrayVec<usize, MAX_CALL_DEPTH>,
    // keep track of the number of objects in the current function to know how many to pop when returning.
    // Index 0 is global context, containing number of global variables + functions
    current_function_objects: ArrayVec<usize, MAX_CALL_DEPTH>,

    // We also need to keep track of loop frames for break/continue statements.
    loop_stack: ArrayVec<LoopFrame, MAX_LOOP_DEPTH>,

    module_resolver: Option<ModuleResolver<'a>>,

    // Expression evaluation stacks
    expression_stack: ArrayVec<EngineObject<'a>, MAX_EXPRESSION_DEPTH>,
    expression_operator_stack: ArrayVec<(Token<'a>, u8), MAX_EXPRESSION_DEPTH>,

    tokenizer: Tokenizer<'a>,
    // TODO: maybe a "scratch space" for e.g. string concatenation / unescapes, so we don't need to allocate for them
}

impl<'a, const STACK_SIZE: usize, const MAX_CALL_DEPTH: usize, const MAX_LOOP_DEPTH: usize>
    VmContext<'a, STACK_SIZE, MAX_CALL_DEPTH, MAX_LOOP_DEPTH>
{
    const _ASSERT_STACK_SIZE: () = assert!(STACK_SIZE > 0, "STACK_SIZE must be greater than 0");
    const _ASSERT_MAX_CALL_DEPTH: () =
        assert!(MAX_CALL_DEPTH > 0, "MAX_CALL_DEPTH must be greater than 0");
    const _ASSERT_MAX_LOOP_DEPTH: () =
        assert!(MAX_LOOP_DEPTH > 0, "MAX_LOOP_DEPTH must be greater than 0");

    pub fn new(script: &'a [u8]) -> Self {
        Self::new_with_modules(script, None)
    }

    pub fn new_with_modules(script: &'a [u8], module_resolver: Option<ModuleResolver<'a>>) -> Self {
        let mut vm = Self {
            stack: ArrayVec::new_const(),
            frame_pointers: ArrayVec::new_const(),
            current_function_objects: ArrayVec::new_const(),
            loop_stack: ArrayVec::new_const(),
            tokenizer: Tokenizer::new(script),
            expression_operator_stack: ArrayVec::new_const(),
            expression_stack: ArrayVec::new_const(),
            module_resolver,
        };
        // global context starts with 0 objects
        unsafe {
            // SAFETY: works since MAX_CALL_DEPTH > 0, asserted above
            vm.current_function_objects.push_unchecked(0);
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
                self.consume_token(&Token::LParen)?;
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
                        Token::RParen if (!next_ident || nargs == 0) => break,
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

                self.consume_token(&Token::LBrace)?;

                self.skip_block()?;

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
                // After if comes an expression
                unimplemented!("if statements")
            }
            (Token::Return, _) => {
                // Empty, separator, or expression
                unimplemented!("return statements")
            }
            (Token::While, _) => {
                // while + condition
                unimplemented!("while")
            }
            (Token::LParen, _) => {
                // Expressions
                unimplemented!("expressions")
            }
            (Token::LBrace, _) => {
                // Blocks
                unimplemented!("blocks")
            }
            (Token::Eof, _) => return Ok(false),
            // Anything else is just an expression, e.g. a function call
            _ => {
                // Expression
                unimplemented!("catch all: {:#?}, {:#?}", first_token, second_token)
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
        // We allow multiple semicolons as statement separators, but they are optional, so we just skip them if they are there.
        let token = self.tokenizer.advance();
        if Token::Separator == token || Token::Eof == token {
            return Ok(());
        } else {
            return Err(InterpreterError::UnexpectedToken {
                expected: Token::Separator,
                found: token,
            });
        }
    }

    pub fn set_var(
        &mut self,
        name: &'a [u8],
        value: EngineObject<'a>,
    ) -> Result<(), InterpreterError<'a>> {
        if self.stack.len() >= STACK_SIZE {
            return Err(InterpreterError::StackOverflow);
        }

        // First, we check if we can find the variable in the current function stack.
        // current_function_objects[frame_ptr] tells us how many variables are in the current function, and we only search those.
        let locals_count = self.current_function_objects[self.frame_pointers.len()];
        let locals_range = ((self.stack.len() - locals_count)..self.stack.len()).rev();

        // Additionally, we also look at the global context (frame 0)
        let globals_range = if self.frame_pointers.len() > 0 {
            (0..self.current_function_objects[0]).rev()
        } else {
            (0..0).rev()
        };

        for i in locals_range.chain(globals_range) {
            if self.stack[i].name == name {
                self.stack[i].value = value;
                return Ok(());
            }
        }

        unsafe {
            // SAFETY: checked at function start
            self.stack.push_unchecked(Variable { name, value });
        };
        self.current_function_objects[self.frame_pointers.len()] += 1;
        Ok(())
    }

    pub fn get_var(
        &mut self,
        name: &'a [u8],
    ) -> Result<&mut EngineObject<'a>, InterpreterError<'a>> {
        // We look for the variable in the current function stack first, then global context if not found.
        let locals_count = self.current_function_objects[self.frame_pointers.len()];
        let locals_range = ((self.stack.len() - locals_count)..self.stack.len()).rev();

        let globals_range = if self.frame_pointers.len() > 0 {
            (0..self.current_function_objects[0]).rev()
        } else {
            (0..0).rev()
        };

        for i in locals_range.chain(globals_range) {
            if self.stack[i].name == name {
                return Ok(&mut self.stack[i].value);
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
                            .map_err(|_| InterpreterError::ExpressionStackExhausted)?;

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
                            .map_err(|_| InterpreterError::ExpressionStackExhausted)?;

                        expect_operand = false;
                    }
                    Token::Identifier(id) => {
                        let var = self.get_var(id)?.clone();
                        self.expression_stack
                            .try_push(var)
                            .map_err(|_| InterpreterError::ExpressionStackExhausted)?;

                        expect_operand = false;
                    }
                    Token::LParen => {
                        // Push sentinel with 0 precedence so nothing pops it until RParen
                        self.expression_operator_stack
                            .try_push((Token::LParen, 0))
                            .map_err(|_| InterpreterError::ExpressionStackExhausted)?;

                        // we don't change expect_operand here!
                    }
                    // Unary operators
                    Token::Bang => {
                        self.expression_operator_stack
                            .try_push((Token::Bang, 255)) // highest precedence for unary ops
                            .map_err(|_| InterpreterError::ExpressionStackExhausted)?;
                    }
                    Token::Plus => {
                        // Can be ignored
                    }
                    Token::Minus => {
                        // push a "0-..." onto the stack, to turn unary minus into binary
                        self.expression_stack
                            .try_push(EngineObject::Int(0))
                            .map_err(|_| InterpreterError::ExpressionStackExhausted)?;
                        self.expression_operator_stack
                            .try_push((Token::Minus, 255)) // highest precedence for unary ops
                            .map_err(|_| InterpreterError::ExpressionStackExhausted)?;
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
                            .map_err(|_| InterpreterError::ExpressionStackExhausted)?;

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
                            .map_err(|_| InterpreterError::ExpressionStackExhausted)?;
                        match self.tokenizer.advance() {
                            Token::Identifier(id) => {
                                // We push the name as a StringLiteral so `pop_and_apply` can use it
                                self.expression_stack
                                    .try_push(EngineObject::MemberAccess { name: id })
                                    .map_err(|_| InterpreterError::ExpressionStackExhausted)?;
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
                    Token::RParen => {
                        // Execute everything back to the LParen
                        while initial_ops_stack_len < self.expression_operator_stack.len()
                            && let Some((top_op, _)) = self.expression_operator_stack.last()
                        {
                            if *top_op == Token::LParen {
                                break;
                            }
                            self.pop_and_apply()?;
                        }

                        if self.expression_operator_stack.pop().map(|(t, _)| t)
                            != Some(Token::LParen)
                        {
                            return Err(InterpreterError::InvalidExpression(Token::RParen));
                        }
                        self.consume_token(&Token::RParen)?;
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
            .ok_or(InterpreterError::InvalidExpression(Token::Eof))
    }

    fn pop_and_apply(&mut self) -> Result<(), InterpreterError<'a>> {
        let (op, _) = self.expression_operator_stack.pop().unwrap();
        let right = self
            .expression_stack
            .pop()
            .ok_or(InterpreterError::InvalidExpression(Token::Eof))?;

        match op {
            // Unary operators
            Token::Bang => {
                if let EngineObject::Bool(b) = right {
                    self.expression_stack
                        .try_push(EngineObject::Bool(!b))
                        .map_err(|_| InterpreterError::ExpressionStackExhausted)?;
                    return Ok(());
                } else if let EngineObject::Int(i) = right {
                    self.expression_stack
                        .try_push(EngineObject::Int(if i == 0 { 1 } else { 0 }))
                        .map_err(|_| InterpreterError::ExpressionStackExhausted)?;
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
            .ok_or(InterpreterError::InvalidExpression(Token::Eof))?;

        match (&mut left, op, &right) {
            (EngineObject::Int(l), Token::Plus, EngineObject::Int(r)) => *l += r,
            (EngineObject::Int(l), Token::Minus, EngineObject::Int(r)) => *l -= r,
            (EngineObject::Int(l), Token::Star, EngineObject::Int(r)) => *l *= r,
            (EngineObject::Int(l), Token::Slash, EngineObject::Int(r)) => *l /= r,

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
            .map_err(|_| InterpreterError::ExpressionStackExhausted)?;
        Ok(())
    }

    fn infix_binding_power(&self, token: &Token) -> Option<(u8, u8)> {
        match token {
            Token::Plus | Token::Minus => Some((1, 2)), // left bp, right bp
            Token::Star | Token::Slash => Some((3, 4)),
            Token::Equals | Token::Lt | Token::Gt | Token::Lte | Token::Gte => Some((0, 1)),
            _ => None,
        }
    }

    fn skip_block(&mut self) -> Result<(), InterpreterError<'a>> {
        let mut depth = 1; // We assume we just passed the opening '{' (or are about to)

        while depth > 0 {
            match self.tokenizer.advance() {
                Token::LBrace => depth += 1,
                Token::RBrace => depth -= 1,
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
        assert!(matches!(vm.run(), Err(InterpreterError::StackOverflow)));
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
                    num_args: 2
                }
            ));
        }

        let test_func2 = vm.get_var(b"test2").expect("function to be variable");
        assert!(matches!(
            test_func2,
            EngineObject::Function {
                position: 52,
                num_args: 0
            }
        ));
    }
}
