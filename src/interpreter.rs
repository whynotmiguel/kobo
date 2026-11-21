use crate::ast::{Expr, Program, Resolved, Stmt};
use crate::environment::Environment;
use crate::token::{Token, TokenType};
use crate::value::Value;

pub struct RuntimeError {
    pub line: usize,
    pub message: String,
}

/// Why a statement stopped early. An error unwinds to `main`; a `return` unwinds only as
/// far as the call that started the function, which is why both travel the same way. Rust
/// gives `?` for free on a `Result`, and a `return` is exactly a non-local exit.
enum Flow {
    Error(RuntimeError),
    Return(Value),
    Break,
}

type Exec<T> = Result<T, Flow>;

/// Kobo's call stack is the Rust call stack: `call` calls `execute` calls `evaluate`
/// calls `call`. Running off the end of it aborts the process with no diagnostic, so the
/// depth is capped instead. Measured on this toolchain, the real limit is near 1,500
/// frames; 500 leaves room for a deeply nested expression inside every one of them.
pub const MAX_FRAMES: usize = 500;

pub fn interpret(program: &Program) -> Result<(), RuntimeError> {
    let mut env = Environment::new();
    for stmt in &program.stmts {
        match execute(stmt, &mut env, program) {
            Ok(()) => {}
            // `return` outside any function ends the program (spec section 3.1).
            Err(Flow::Return(_)) => return Ok(()),
            Err(Flow::Error(e)) => return Err(e),
            Err(Flow::Break) => {
                return Err(RuntimeError {
                    line: 0,
                    message: "Break statement outside of loop".to_string(),
                });
            }
        }
    }
    Ok(())
}

fn execute(stmt: &Stmt, env: &mut Environment, program: &Program) -> Exec<()> {
    match stmt {
        Stmt::Expression(e) => {
            evaluate(e, env, program)?;
            Ok(())
        }
        Stmt::Print(e) => {
            let value = evaluate(e, env, program)?;
            println!("{}", value);
            Ok(())
        }
        Stmt::Var { name, initialiser, slot } => {
            let value = match initialiser {
                Some(e) => evaluate(e, env, program)?,
                None => Value::Nil,
            };
            if *slot == None {
                // Global
                env.declare(&name.lexeme, value);
            } else {
                // Local - already on stack in the resolver's slot
                // The environment's declare will put it in the current scope
                env.declare(&name.lexeme, value);
            }
            Ok(())
        }
        Stmt::Block { body, .. } => {
            env.push_scope();
            for stmt in body {
                execute(stmt, env, program)?;
            }
            env.pop_scope();
            Ok(())
        }
        Stmt::If { condition, then_branch, else_branch } => {
            let cond_value = evaluate(condition, env, program)?;
            if cond_value.is_truthy() {
                execute(then_branch, env, program)?;
            } else if let Some(alt) = else_branch {
                execute(alt, env, program)?;
            }
            Ok(())
        }
        Stmt::While { condition, body } => {
            loop {
                let cond_value = evaluate(condition, env, program)?;
                if !cond_value.is_truthy() {
                    break;
                }
                match execute(body, env, program) {
                    Ok(()) => {}
                    Err(Flow::Break) => break,
                    Err(e) => return Err(e),
                }
            }
            Ok(())
        }
        Stmt::Break(_) => {
            Err(Flow::Break)
        }
        Stmt::Function { index, name, slot } => {
            let func_value = Value::Function {
                index: *index,
                name: name.clone(),
            };
            if *slot == None {
                // Global
                env.declare(name, func_value);
            } else {
                env.declare(name, func_value);
            }
            Ok(())
        }
        Stmt::Return(value) => {
            let return_value = match value {
                Some(e) => evaluate(e, env, program)?,
                None => Value::Nil,
            };
            Err(Flow::Return(return_value))
        }
    }
}

fn evaluate(expr: &Expr, env: &mut Environment, program: &Program) -> Exec<Value> {
    match expr {
        Expr::Literal { value, .. } => Ok(value.clone()),
        Expr::Grouping(inner) => evaluate(inner, env, program),
        Expr::Variable { name, at } => {
            match *at {
                Resolved::Global => {
                    env.get_global(&name.lexeme)
                        .cloned()
                        .ok_or_else(|| undefined(name))
                }
                Resolved::Local { depth, slot: _ } => {
                    env.get_at(depth, &name.lexeme)
                        .cloned()
                        .ok_or_else(|| undefined(name))
                }
            }
        }
        Expr::Assign { name, value, at } => {
            let val = evaluate(value, env, program)?;
            match *at {
                Resolved::Global => {
                    if !env.assign_global(&name.lexeme, val.clone()) {
                        return Err(undefined(name));
                    }
                }
                Resolved::Local { depth, slot: _ } => {
                    if !env.assign_at(depth, &name.lexeme, val.clone()) {
                        return Err(undefined(name));
                    }
                }
            }
            Ok(val)
        }
        Expr::Call { callee, paren, args } => {
            let callee_val = evaluate(callee, env, program)?;
            let mut arg_vals = Vec::new();
            for arg in args {
                arg_vals.push(evaluate(arg, env, program)?);
            }
            call_callee(callee_val, arg_vals, paren.line, env, program)
        }
        Expr::Unary { op, right } => {
            let right_val = evaluate(right, env, program)?;
            match op.kind {
                TokenType::Minus => {
                    if let Value::Number(n) = right_val {
                        Ok(Value::Number(-n))
                    } else {
                        Err(err(op.line, "Operand must be a number."))
                    }
                }
                TokenType::Bang => {
                    Ok(Value::Bool(!right_val.is_truthy()))
                }
                _ => Err(err(op.line, "Invalid unary operator.")),
            }
        }
        Expr::Binary { left, op, right } => {
            let left_val = evaluate(left, env, program)?;
            let right_val = evaluate(right, env, program)?;
            match op.kind {
                TokenType::Plus => {
                    match (left_val, right_val) {
                        (Value::Number(a), Value::Number(b)) => Ok(Value::Number(a + b)),
                        (Value::Str(a), Value::Str(b)) => Ok(Value::Str(a + &b)),
                        _ => Err(err(op.line, "Operands must be two numbers or two strings.")),
                    }
                }
                TokenType::Minus => {
                    let (a, b) = numbers(left_val, right_val, op.line)?;
                    Ok(Value::Number(a - b))
                }
                TokenType::Star => {
                    let (a, b) = numbers(left_val, right_val, op.line)?;
                    Ok(Value::Number(a * b))
                }
                TokenType::Slash => {
                    let (a, b) = numbers(left_val, right_val, op.line)?;
                    if b == 0.0 {
                        Err(err(op.line, "Division by zero."))
                    } else {
                        Ok(Value::Number(a / b))
                    }
                }
                TokenType::Greater => {
                    let (a, b) = numbers(left_val, right_val, op.line)?;
                    Ok(Value::Bool(a > b))
                }
                TokenType::GreaterEqual => {
                    let (a, b) = numbers(left_val, right_val, op.line)?;
                    Ok(Value::Bool(a >= b))
                }
                TokenType::Less => {
                    let (a, b) = numbers(left_val, right_val, op.line)?;
                    Ok(Value::Bool(a < b))
                }
                TokenType::LessEqual => {
                    let (a, b) = numbers(left_val, right_val, op.line)?;
                    Ok(Value::Bool(a <= b))
                }
                TokenType::EqualEqual => Ok(Value::Bool(values_equal(&left_val, &right_val))),
                TokenType::BangEqual => Ok(Value::Bool(!values_equal(&left_val, &right_val))),
                _ => Err(err(op.line, "Invalid binary operator.")),
            }
        }
        Expr::Logical { left, op, right } => {
            let left_val = evaluate(left, env, program)?;
            match op.kind {
                TokenType::Or => {
                    if left_val.is_truthy() {
                        Ok(left_val)
                    } else {
                        evaluate(right, env, program)
                    }
                }
                TokenType::And => {
                    if !left_val.is_truthy() {
                        Ok(left_val)
                    } else {
                        evaluate(right, env, program)
                    }
                }
                _ => Err(err(op.line, "Invalid logical operator.")),
            }
        }
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

fn call_callee(callee: Value, args: Vec<Value>, line: usize, env: &mut Environment, program: &Program) -> Exec<Value> {
    match callee {
        Value::Function { index, .. } => {
            call(index, args, line, env, program)
        }
        _ => Err(err(line, "This value is not a function.")),
    }
}

/// Run a function body in a fresh frame. Everything about Kobo's scope rules is in these
/// twenty lines: the parameters are declared in the frame's own scope, the body can reach
/// past it only to the globals, and the frame is popped on every path out: normal
/// completion, `return`, or a runtime error.
fn call(
    index: usize,
    args: Vec<Value>,
    line: usize,
    env: &mut Environment,
    program: &Program,
) -> Exec<Value> {
    let function = &program.functions[index];

    // Check arity
    if args.len() != function.params.len() {
        return Err(err(line, &format!("Expected {} arguments but got {}.", function.params.len(), args.len())));
    }

    // Check stack overflow
    if env.frame_depth() >= MAX_FRAMES {
        return Err(err(line, "Stack overflow."));
    }

    // Push a new frame
    env.push_frame();

    // Declare parameters in the new frame
    for (param, arg) in function.params.iter().zip(args.into_iter()) {
        env.declare(param, arg);
    }

    // Run the function body
    let result = match execute_block(&function.body, env, program) {
        Ok(()) => Ok(Value::Nil),
        Err(Flow::Return(value)) => Ok(value),
        Err(Flow::Error(e)) => Err(Flow::Error(e)),
        Err(Flow::Break) => Err(Flow::Error(RuntimeError {
            line: function.line,
            message: "Break statement outside of loop".to_string(),
        })),
    };

    // Pop the frame on all paths out
    env.pop_frame();

    result
}

fn execute_block(stmts: &[Stmt], env: &mut Environment, program: &Program) -> Exec<()> {
    for stmt in stmts {
        execute(stmt, env, program)?;
    }
    Ok(())
}

fn numbers(l: Value, r: Value, line: usize) -> Exec<(f64, f64)> {
    match (l, r) {
        (Value::Number(a), Value::Number(b)) => Ok((a, b)),
        _ => Err(err(line, "Operands must be numbers.")),
    }
}

fn arith(l: Value, r: Value, line: usize, f: fn(f64, f64) -> f64) -> Exec<Value> {
    let (a, b) = numbers(l, r, line)?;
    Ok(Value::Number(f(a, b)))
}

fn compare(l: Value, r: Value, line: usize, f: fn(f64, f64) -> bool) -> Exec<Value> {
    let (a, b) = numbers(l, r, line)?;
    Ok(Value::Bool(f(a, b)))
}

fn undefined(name: &Token) -> Flow {
    err(name.line, &format!("Variable '{}' is not defined.", name.lexeme))
}

fn err(line: usize, message: &str) -> Flow {
    Flow::Error(RuntimeError {
        line,
        message: message.to_string(),
    })
}
