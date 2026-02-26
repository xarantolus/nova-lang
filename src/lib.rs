#![cfg_attr(not(test), no_std)]

use core::ops::Range;

use crate::tokenizer::{Token, Tokenizer};

mod tokenizer;

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum EngineObject<'a> {
    // Position of the function in the script. We can jump to it to call it.
    Function(u32),
    // A simple integer value.
    Int(u32),
    // A string literal. Note that it is still escaped, so we need to unescape it before using it.
    // TODO: maybe include info on whether to escape somehow, or even check that in the tokenizer?
    StringLiteral(&'a [u8]),
    Unit,
}

#[cfg(test)]
impl<'a> Into<EngineObject<'a>> for u32 {
    fn into(self) -> EngineObject<'a> {
        EngineObject::Int(self)
    }
}

#[cfg(test)]
impl Into<u32> for EngineObject<'_> {
    fn into(self) -> u32 {
        match self {
            EngineObject::Int(i) => i,
            _ => panic!("Expected Int"),
        }
    }
}

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum InterpreterError<'a> {
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

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
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
    // locals[0] == global context. We just walk up the stack to find variables, no separate call frames.
    stack: [Variable<'a>; STACK_SIZE],
    stack_ptr: usize,

    // We keep track of frame pointers to know where to return to after a function call.
    frame_pointers: [usize; MAX_CALL_DEPTH],
    // keep track of the number of objects in the current function to know how many to pop when returning.
    // Index 0 is global context, containing number of global variables + functions
    current_function_objects: [usize; MAX_CALL_DEPTH],
    frame_ptr: usize,

    loop_stack: [LoopFrame; MAX_LOOP_DEPTH],
    loop_depth: usize,

    tokenizer: Tokenizer<'a>,
    // TODO: maybe a "scratch space" for e.g. string concatenation / unescapes, so we don't need to allocate for them
}

impl<'a, const STACK_SIZE: usize, const MAX_CALL_DEPTH: usize, const MAX_LOOP_DEPTH: usize>
    VmContext<'a, STACK_SIZE, MAX_CALL_DEPTH, MAX_LOOP_DEPTH>
{
    pub const fn new(script: &'a [u8]) -> Self {
        Self {
            stack: [Variable {
                name: &[],
                value: EngineObject::Unit,
            }; STACK_SIZE],
            stack_ptr: 0,

            frame_pointers: [0; MAX_CALL_DEPTH],
            current_function_objects: [0; MAX_CALL_DEPTH],
            frame_ptr: 0,

            loop_stack: [LoopFrame {
                end_cursor: None,
                start_cursor: 0,
            }; MAX_LOOP_DEPTH],
            loop_depth: 0,

            tokenizer: Tokenizer::new(script),
        }
    }

    pub fn run(&mut self) -> Result<(), InterpreterError> {
        while self.step().is_ok() {}
        Ok(())
    }

    // Returns: Ok(true) if work was done, Ok(false) if EOF, Err on error
    pub fn step(&mut self) -> Result<bool, &'static str> {
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
        if self.stack_ptr >= STACK_SIZE {
            return Err(InterpreterError::StackOverflow);
        }

        // First, we check if we can find the variable in the current function stack.
        // current_function_objects[frame_ptr] tells us how many variables are in the current function, and we only search those.
        let locals_count = self.current_function_objects[self.frame_ptr];
        let locals_range = ((self.stack_ptr - locals_count)..self.stack_ptr).rev();

        // Additionally, we also look at the global context (frame 0)
        let globals_range = if self.frame_ptr > 0 {
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

        self.stack[self.stack_ptr] = Variable { name, value };
        self.stack_ptr += 1;
        self.current_function_objects[self.frame_ptr] += 1;
        Ok(())
    }

    // TODO: maybe return error
    fn get_var(&mut self, name: &'a [u8]) -> Option<&mut EngineObject<'a>> {
        // We look for the variable in the current function stack first, then global context if not found.
        let locals_count = self.current_function_objects[self.frame_ptr];
        let locals_range = ((self.stack_ptr - locals_count)..self.stack_ptr).rev();

        let globals_range = if self.frame_ptr > 0 {
            (0..self.current_function_objects[0]).rev()
        } else {
            (0..0).rev()
        };

        for i in locals_range.chain(globals_range) {
            if self.stack[i].name == name {
                return Some(&mut self.stack[i].value);
            }
        }

        None
    }

    fn eval_expr(&mut self, min_bp: u8) -> Result<EngineObject<'a>, InterpreterError<'a>> {
        unimplemented!()
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

    #[test]
    fn test_simple_expr() {
        let mut vm: VmContext<'_> = VmContext::new(b"1 + 2 * 3");
        assert_eq!(vm.eval_expr(0).unwrap(), 7);
    }

    #[test]
    fn test_simple_expr() {
        let mut vm: VmContext<'_> = VmContext::new(b"1 + 2 * 3");
        assert_eq!(vm.eval_expr(0).unwrap(), 7);
    }
}
