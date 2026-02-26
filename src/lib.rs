#![cfg_attr(not(test), no_std)]

use core::any::Any;
use core::cell::RefCell;
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
    // Position of the function in the script. We can jump to it to call it.
    Function(u32),
    // A simple integer value.
    Int(u32),
    // A string literal.
    // If it contains escape characters, we have to unescape it before using
    StringLiteral {
        content: &'a [u8],
        has_escape_characters: bool,
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
            (EngineObject::Int(a), EngineObject::Int(b)) => a == b,
            (
                EngineObject::StringLiteral { content: a, .. },
                EngineObject::StringLiteral { content: b, .. },
            ) => a == b,
            _ => false,
        }
    }
}

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
            EngineObject::Function(pos) => write!(f, "<function@{}>", pos),
            EngineObject::Handle { id, .. } => write!(f, "<handle@{}>", id),
            EngineObject::Unit => write!(f, "void"),
        }
    }
}

impl core::fmt::Debug for EngineObject<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        <Self as core::fmt::Display>::fmt(self, f)
    }
}

impl<'a> Into<EngineObject<'a>> for u32 {
    fn into(self) -> EngineObject<'a> {
        EngineObject::Int(self)
    }
}

impl Into<u32> for EngineObject<'_> {
    fn into(self) -> u32 {
        match self {
            EngineObject::Int(i) => i,
            _ => panic!("Expected Int"),
        }
    }
}

#[derive(Debug, PartialEq, Clone)]
pub enum InterpreterError<'a> {
    NameError(&'a [u8]),
    InvalidOperation {
        op: Token<'a>,
        left: EngineObject<'a>,
        right: EngineObject<'a>,
    },
    InvalidExpression(Token<'a>),
    UnexpectedToken {
        expected: Token<'a>,
        found: Token<'a>,
    },
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

    tokenizer: Tokenizer<'a>,
    // TODO: maybe a "scratch space" for e.g. string concatenation / unescapes, so we don't need to allocate for them
}

impl<'a, const STACK_SIZE: usize, const MAX_CALL_DEPTH: usize, const MAX_LOOP_DEPTH: usize>
    VmContext<'a, STACK_SIZE, MAX_CALL_DEPTH, MAX_LOOP_DEPTH>
{
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
            module_resolver,
        };
        // global context starts with 0 objects
        vm.current_function_objects.push(0);
        vm
    }

    pub fn run(&mut self) -> Result<(), InterpreterError<'a>> {
        while self.step().is_ok() {}
        Ok(())
    }

    // Returns: Ok(true) if work was done, Ok(false) if EOF, Err on error
    pub fn step(&mut self) -> Result<bool, InterpreterError<'a>> {
        // 1. Peek at the next token to decide the statement type
        let token = self.tokenizer.peek();

        // match token {}
        unimplemented!()
    }

    /// Consumes next tokens, ensuring it is the expected one, otherwise returns an error.
    fn consume_token(&mut self, expected: Token<'a>) -> Result<(), InterpreterError<'a>> {
        let token = self.tokenizer.advance();
        if token == expected {
            Ok(())
        } else {
            Err(InterpreterError::UnexpectedToken {
                expected,
                found: token,
            })
        }
    }

    /// Consumes separator tokens (optional!)
    fn consume_separator(&mut self) {
        // We allow multiple semicolons as statement separators, but they are optional, so we just skip them if they are there.
        while self.tokenizer.peek() == Token::Separator {
            self.tokenizer.advance();
        }
    }

    fn set_var(
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

        self.stack.push(Variable { name, value });
        self.current_function_objects[self.frame_pointers.len()] += 1;
        Ok(())
    }

    // TODO: maybe return error
    fn get_var(&mut self, name: &'a [u8]) -> Result<&mut EngineObject<'a>, InterpreterError<'a>> {
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

    fn eval_expr(&mut self, min_bp: u8) -> Result<EngineObject<'a>, InterpreterError<'a>> {
        let mut left = match self.tokenizer.advance() {
            Token::IntegerLit(i) => EngineObject::Int(i),
            Token::StringLit {
                content,
                has_escape_characters,
            } => EngineObject::StringLiteral {
                content,
                has_escape_characters,
            },
            Token::Identifier(id) => self.get_var(id)?.clone(),
            Token::LParen => {
                let expr = self.eval_expr(0)?;
                self.consume_token(Token::RParen)?;
                expr
            }
            token => {
                return Err(InterpreterError::InvalidExpression(token));
            }
        };

        Ok(loop {
            match self.tokenizer.peek() {
                Token::Equals
                | Token::Lt
                | Token::Gt
                | Token::Lte
                | Token::Gte
                | Token::Plus
                | Token::Minus
                | Token::Star
                | Token::Slash => {
                    let op = self.tokenizer.peek();
                    let (_, right_bp) = match self.infix_binding_power(&op) {
                        Some((left_b, right_b)) if left_b >= min_bp => (left_b, right_b),
                        _ => break left,
                    };
                    let op = self.tokenizer.advance();
                    let right = self.eval_expr(right_bp)?;

                    match (left, op, right) {
                        (EngineObject::Int(lhs), Token::Plus, EngineObject::Int(rhs)) => {
                            left = EngineObject::Int(lhs + rhs);
                        }
                        (EngineObject::Int(lhs), Token::Minus, EngineObject::Int(rhs)) => {
                            left = EngineObject::Int(lhs - rhs);
                        }
                        (EngineObject::Int(lhs), Token::Star, EngineObject::Int(rhs)) => {
                            left = EngineObject::Int(lhs * rhs);
                        }
                        (EngineObject::Int(lhs), Token::Slash, EngineObject::Int(rhs)) => {
                            left = EngineObject::Int(lhs / rhs);
                        }
                        (left, op, right) => {
                            return Err(InterpreterError::InvalidOperation { op, left, right });
                        }
                    }
                }
                Token::Dot => {
                    match left {
                        EngineObject::Module(module) => {
                            // We need to resolve the module and look up the member.
                            let module = module.borrow_mut();
                            let member_name = match self.tokenizer.advance() {
                                Token::Identifier(id) => id,
                                token => {
                                    return Err(InterpreterError::UnexpectedToken {
                                        expected: Token::Identifier(&[]),
                                        found: token,
                                    });
                                }
                            };
                            // TODO: parse function arguments as expressions
                        }
                        _ => {
                            return Err(InterpreterError::InvalidExpression(Token::Dot));
                        }
                    }

                    unimplemented!("member access not implemented yet");
                }
                Token::LParen => {
                    unimplemented!("function calls not implemented yet");
                }
                _ => break left,
            }
        })
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
        assert_eq!(vm.eval_expr(0).unwrap(), 7.into());
    }

    #[test]
    fn test_simple_expr2() {
        let mut vm: VmContext<'_> = VmContext::new(b"2 * 3 + 1");
        assert_eq!(vm.eval_expr(0).unwrap(), 7.into());
    }

    #[test]
    fn long_expression() {
        let mut vm: VmContext<'_> = VmContext::new(b"1 + 2 + 3 * 8 + 4 + 5");
        assert_eq!(vm.eval_expr(0).unwrap(), 36.into());
    }

    #[test]
    fn parens_expression() {
        let mut vm: VmContext<'_> = VmContext::new(b"(1 + 2 + 3) * (8 + 4 + 5)");
        assert_eq!(vm.eval_expr(0).unwrap(), 102.into());
    }

    #[test]
    fn parens_nested() {
        let mut vm: VmContext<'_> = VmContext::new(b"((1 + 2) * (3 + 4)) * 5");
        assert_eq!(vm.eval_expr(0).unwrap(), 105.into());
    }

    #[test]
    fn parens_nested2() {
        let mut vm: VmContext<'_> = VmContext::new(b"(1 * (2 * (3 * 4))) * 5");
        assert_eq!(vm.eval_expr(0).unwrap(), 120.into());
    }

    #[test]
    fn parens_nested3() {
        let mut vm: VmContext<'_> = VmContext::new(b"5 * (4 * (3 * (2 * 1)))");
        assert_eq!(vm.eval_expr(0).unwrap(), 120.into());
    }
}
