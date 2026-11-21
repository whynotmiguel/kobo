use std::collections::HashMap;

use crate::chunk::{Bytecode, Chunk, Op};
// The same error type and the same frame limit as the tree-walker, deliberately shared.
// Spec section 5 fixes both, and the two engines are required to produce identical output.
use crate::interpreter::{RuntimeError, MAX_FRAMES};
use crate::value::Value;

/// One active call: which chunk it is running, how far through it is, and where its slice
/// of the stack begins. This is the activation record of week 13's Module A, made of two
/// `usize`s and an index; there is nothing else in it, because Kobo has no closures.
struct Frame {
    /// An index into `Bytecode::functions`, or `None` for the script itself.
    function: Option<usize>,
    ip: usize,
    /// Stack index of local slot 0. The callee's own value sits just below it.
    base: usize,
}

impl Frame {
    fn line(&self, chunk: &Chunk) -> usize {
        if self.ip < chunk.lines.len() {
            chunk.lines[self.ip]
        } else {
            chunk.last_line()
        }
    }
}

/// Run compiled bytecode. The VM owns it: nothing needs the chunks afterwards, and owning
/// them keeps a lifetime out of every signature in this file.
pub fn run(bytecode: Bytecode) -> Result<(), RuntimeError> {
    let mut vm = Vm {
        bytecode,
        stack: Vec::new(),
        globals: HashMap::new(),
        frames: vec![Frame {
            function: None,
            ip: 0,
            base: 0,
        }],
    };
    vm.run()
}

struct Vm {
    bytecode: Bytecode,
    /// Values in flight and the locals of every active call, in one contiguous vector.
    /// The interpreter kept those two things apart: a `Vec` of `HashMap`s for the names
    /// and the Rust stack for the arithmetic. Merging them is what makes this fast.
    stack: Vec<Value>,
    globals: HashMap<String, Value>,
    frames: Vec<Frame>,
}

impl Vm {
    fn run(&mut self) -> Result<(), RuntimeError> {
        loop {
            // Get current frame info without holding borrow across mutable operations
            let (function, ip, base) = {
                let frame = self.frames.last().expect("a frame is always running");
                (frame.function, frame.ip, frame.base)
            };

            let chunk = match function {
                None => &self.bytecode.script,
                Some(i) => &self.bytecode.functions[i],
            };

            if ip >= chunk.code.len() {
                return Err(error(
                    if ip < chunk.lines.len() { chunk.lines[ip] } else { chunk.last_line() },
                    "Instruction pointer past end of chunk."
                ));
            }

            let op = chunk.code[ip].clone();
            let line = chunk.lines[ip];

            // Increment IP before executing (so jumps work correctly)
            self.frames.last_mut().expect("a frame is always running").ip += 1;

            match op {
                Op::Constant(idx) => {
                    let value = chunk.constants[idx].clone();
                    self.stack.push(value);
                }
                Op::Nil => self.stack.push(Value::Nil),
                Op::True => self.stack.push(Value::Bool(true)),
                Op::False => self.stack.push(Value::Bool(false)),
                Op::Pop => { self.stack.pop(); }
                Op::DefineGlobal(idx) => {
                    let name = chunk.constants[idx].to_string();
                    let value = self.stack.pop().expect("stack balanced");
                    self.globals.insert(name, value);
                }
                Op::GetGlobal(idx) => {
                    let name = chunk.constants[idx].to_string();
                    let value = self.globals.get(&name)
                        .cloned()
                        .ok_or_else(|| undefined(line, &name))?;
                    self.stack.push(value);
                }
                Op::SetGlobal(idx) => {
                    let name = chunk.constants[idx].to_string();
                    let value = self.stack.last().expect("stack balanced").clone();
                    if !self.globals.contains_key(&name) {
                        return Err(undefined(line, &name));
                    }
                    self.globals.insert(name, value);
                }
                Op::GetLocal(slot) => {
                    let value = self.stack[base + slot].clone();
                    self.stack.push(value);
                }
                Op::SetLocal(slot) => {
                    let value = self.stack.last().expect("stack balanced").clone();
                    self.stack[base + slot] = value;
                }
                Op::Equal => {
                    let (a, b) = self.pop_two();
                    self.stack.push(Value::Bool(values_equal(&a, &b)));
                }
                Op::NotEqual => {
                    let (a, b) = self.pop_two();
                    self.stack.push(Value::Bool(!values_equal(&a, &b)));
                }
                Op::Greater => self.compare(line, |a, b| a > b)?,
                Op::GreaterEqual => self.compare(line, |a, b| a >= b)?,
                Op::Less => self.compare(line, |a, b| a < b)?,
                Op::LessEqual => self.compare(line, |a, b| a <= b)?,
                Op::Add => self.binary_value(|a, b| {
                    match (a, b) {
                        (Value::Number(a), Value::Number(b)) => Value::Number(a + b),
                        (Value::Str(a), Value::Str(b)) => Value::Str(a + &b),
                        _ => unreachable!("Type checking done at compile time"),
                    }
                }),
                Op::Subtract => self.arith(line, |a, b| a - b)?,
                Op::Multiply => self.arith(line, |a, b| a * b)?,
                Op::Divide => {
                    let (a, b) = self.numbers(line)?;
                    if b == 0.0 {
                        return Err(error(line, "Division by zero."));
                    }
                    self.stack.push(Value::Number(a / b));
                }
                Op::Not => {
                    let value = self.stack.pop().expect("stack balanced");
                    self.stack.push(Value::Bool(!value.is_truthy()));
                }
                Op::Negate => {
                    if let Value::Number(n) = self.stack.pop().expect("stack balanced") {
                        self.stack.push(Value::Number(-n));
                    } else {
                        return Err(error(line, "Operand must be a number."));
                    }
                }
                Op::Print => {
                    let value = self.stack.pop().expect("stack balanced");
                    println!("{}", value);
                }
                Op::Jump(target) => {
                    self.frames.last_mut().expect("a frame is always running").ip = target;
                }
                Op::JumpIfFalse(target) => {
                    let condition = self.stack.pop().expect("stack balanced");
                    if !condition.is_truthy() {
                        self.frames.last_mut().expect("a frame is always running").ip = target;
                    }
                }
                Op::Call(argc) => {
                    self.call(argc, line)?;
                }
                Op::Return => {
                    let value = self.stack.pop().expect("stack balanced");
                    let frame = self.frames.pop().expect("frame exists");
                    if self.frames.is_empty() {
                        return Ok(());
                    }
                    // Truncate stack to remove callee (at base-1) and arguments/locals
                    // frame.base points to first argument (callee_idx + 1)
                    // So callee is at base - 1
                    self.stack.truncate(frame.base - 1);
                    self.stack.push(value);
                }
            }
        }
    }

    /// The callee is on the stack underneath its arguments, put there by the code that
    /// evaluated it. Nothing is moved: the arguments are already lying where the new
    /// frame's slots need them to be.
    fn call(&mut self, argc: usize, line: usize) -> Result<(), RuntimeError> {
        let callee_idx = self.stack.len() - argc - 1;
        let callee = self.stack[callee_idx].clone();

        match callee {
            Value::Function { index, .. } => {
                let function = &self.bytecode.functions[index];

                if argc != function.arity {
                    return Err(error(line, &format!("Expected {} arguments but got {}.", function.arity, argc)));
                }

                if self.frames.len() >= MAX_FRAMES {
                    return Err(error(line, "Stack overflow."));
                }

                // Arguments start at callee_idx + 1, so frame base = callee_idx + 1
                let base = callee_idx + 1;

                self.frames.push(Frame {
                    function: Some(index),
                    ip: 0,
                    base,
                });

                Ok(())
            }
            _ => Err(error(line, "This value is not a function.")),
        }
    }

    // --- primitives -------------------------------------------------------------------

    fn pop_two(&mut self) -> (Value, Value) {
        let b = self.stack.pop().expect("stack balanced");
        let a = self.stack.pop().expect("stack balanced");
        (a, b)
    }

    fn binary_value(&mut self, f: fn(Value, Value) -> Value) {
        let (a, b) = self.pop_two();
        self.stack.push(f(a, b));
    }

    fn numbers(&mut self, line: usize) -> Result<(f64, f64), RuntimeError> {
        match self.pop_two() {
            (Value::Number(a), Value::Number(b)) => Ok((a, b)),
            _ => Err(error(line, "Operands must be numbers.")),
        }
    }

    fn arith(&mut self, line: usize, f: fn(f64, f64) -> f64) -> Result<(), RuntimeError> {
        let (a, b) = self.numbers(line)?;
        self.stack.push(Value::Number(f(a, b)));
        Ok(())
    }

    fn compare(&mut self, line: usize, f: fn(f64, f64) -> bool) -> Result<(), RuntimeError> {
        let (a, b) = self.numbers(line)?;
        self.stack.push(Value::Bool(f(a, b)));
        Ok(())
    }
}

fn values_equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Nil, Value::Nil) => true,
        (Value::Bool(a), Value::Bool(b)) => a == b,
        (Value::Number(a), Value::Number(b)) => a == b,
        (Value::Str(a), Value::Str(b)) => a == b,
        (Value::Function { name: a, .. }, Value::Function { name: b, .. }) => a == b,
        _ => false,
    }
}

fn undefined(line: usize, name: &str) -> RuntimeError {
    error(line, &format!("Variable '{}' is not defined.", name))
}

fn error(line: usize, message: &str) -> RuntimeError {
    RuntimeError {
        line,
        message: message.to_string(),
    }
}