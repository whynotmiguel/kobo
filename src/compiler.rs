use crate::ast::{Expr, Program, Resolved, Stmt};
use crate::chunk::{Bytecode, Chunk, Op};
use crate::token::{Token, TokenType};
use crate::value::Value;

/// Extract the source line from an expression.
fn expr_line(expr: &Expr) -> usize {
    match expr {
        Expr::Literal { line, .. } => *line,
        Expr::Grouping(inner) => expr_line(inner),
        Expr::Variable { name, .. } => name.line,
        Expr::Assign { name, .. } => name.line,
        Expr::Call { paren, .. } => paren.line,
        Expr::Unary { op, .. } => op.line,
        Expr::Binary { op, .. } => op.line,
        Expr::Logical { op, .. } => op.line,
    }
}

/// Extract the source line from a statement.
fn stmt_line(stmt: &Stmt) -> usize {
    match stmt {
        Stmt::Expression(e) | Stmt::Print(e) => expr_line(e),
        Stmt::Var { name, .. } => name.line,
        Stmt::Block { body, .. } => body.first().map(stmt_line).unwrap_or(0),
        Stmt::If { condition, .. } => expr_line(condition),
        Stmt::While { condition, .. } => expr_line(condition),
        Stmt::Function { index, .. } => 0, // Will use function's line from program
        Stmt::Return(value) => value.as_ref().map(expr_line).unwrap_or(0),
        Stmt::Break(token) => token.line,
    }
}

/// AST → bytecode. One pass, no intermediate graph: the tree is already the structure a
/// code generator wants, so walking it and emitting as we go is enough for a language
/// this size.
///
/// **Name resolution is not done here any more.** `resolver.rs` runs first and writes
/// every local's stack slot into the tree, so this file emits `GET_LOCAL 3` by reading a
/// number rather than by keeping its own scope stack and searching it. That used to be
/// two implementations of one rule, the resolver's and this one's, which had to agree
/// and had nothing checking that they did.
pub fn compile(program: &Program) -> Bytecode {
    let functions = program
        .functions
        .iter()
        .map(|f| {
            // Parameters are the frame's first locals, in order, so argument `i` is
            // already in slot `i` when the body starts; the resolver gave them those
            // slots and this compiler never has to know it.
            let mut c = Compiler::new(Chunk::new(f.name.clone(), f.params.len()), program);
            c.in_function = true;
            c.statements(&f.body);
            c.finish(f.line)
        })
        .collect();

    let mut script = Compiler::new(Chunk::new("script".to_string(), 0), program);
    script.statements(&program.stmts);
    let line = script.chunk.last_line();
    Bytecode {
        script: script.finish(line),
        functions,
    }
}

struct Compiler<'a> {
    chunk: Chunk,
    program: &'a Program,
    /// A function body's declarations are locals even at its outermost level; the
    /// script's are globals. That is the only thing this file still needs to know about
    /// scope, and the resolver decides everything else.
    in_function: bool,
    /// Stack of break jump positions for each loop level. Each loop pushes a new vec
    /// to collect break jumps; when the loop ends, all collected jumps are patched to
    /// point to the instruction after the loop.
    loop_breaks: Vec<Vec<usize>>,
}

impl<'a> Compiler<'a> {
    fn new(chunk: Chunk, program: &'a Program) -> Self {
        Compiler {
            chunk,
            program,
            in_function: false,
            loop_breaks: Vec::new(),
        }
    }

    /// Every chunk ends by returning `nil`, so falling off the end of a function body
    /// returns `nil` (spec section 4.3) without the VM needing a special case.
    fn finish(mut self, line: usize) -> Chunk {
        self.chunk.write(Op::Nil, line);
        self.chunk.write(Op::Return, line);
        self.chunk
    }

    fn statements(&mut self, stmts: &[Stmt]) {
        for stmt in stmts {
            self.statement(stmt);
        }
    }

    // --- statements -------------------------------------------------------------------

    fn statement(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::Expression(e) => {
                self.expression(e);
                self.chunk.write(Op::Pop, expr_line(e));
            }
            Stmt::Print(e) => {
                self.expression(e);
                self.chunk.write(Op::Print, expr_line(e));
            }
            Stmt::Var { name, initialiser, slot } => {
                let line = name.line;
                match initialiser {
                    Some(e) => {
                        self.expression(e);
                    }
                    None => {
                        self.chunk.write(Op::Nil, line);
                    }
                }
                self.declare(&name.lexeme, *slot, line);
            }
            Stmt::Block { body, locals } => {
                self.statements(body);
                self.end_scope(*locals);
            }
            Stmt::If { condition, then_branch, else_branch } => {
                let line = expr_line(condition);
                self.expression(condition);
                // Emit JumpIfFalse with placeholder, remember position
                let then_jump = self.chunk.write(Op::JumpIfFalse(0), line);

                self.statement(then_branch);

                // Emit Jump over else branch
                let else_jump = self.chunk.write(Op::Jump(0), line);

                // Patch the JumpIfFalse to point to else branch (or after if)
                self.patch(then_jump);

                if let Some(alt) = else_branch {
                    self.statement(alt);
                }

                // Patch the Jump to point to after the if statement
                self.patch(else_jump);
            }
            Stmt::While { condition, body } => {
                let line = expr_line(condition);
                let loop_start = self.chunk.code.len();

                self.expression(condition);

                // JumpIfFalse to exit loop
                let exit_jump = self.chunk.write(Op::JumpIfFalse(0), line);

                // Push new loop level to track break jumps
                self.loop_breaks.push(Vec::new());

                self.statement(body);

                // Jump back to loop start
                self.chunk.write(Op::Jump(loop_start), line);

                // Patch the exit jump
                self.patch(exit_jump);

                // Pop loop level and patch all break jumps to point after the loop
                let break_jumps = self.loop_breaks.pop().unwrap();
                let after_loop = self.chunk.code.len();
                for jump_pos in break_jumps {
                    if let Op::Jump(_) = self.chunk.code[jump_pos] {
                        self.chunk.code[jump_pos] = Op::Jump(after_loop);
                    }
                }
            }
            Stmt::Function { name, slot, index } => {
                // Find the function's line from the program
                let func_line = self.program.functions[*index].line;
                // Push the function value onto the stack
                let func_value = Value::Function {
                    index: *index,
                    name: name.clone(),
                };
                let func_idx = self.chunk.constant(func_value);
                self.chunk.write(Op::Constant(func_idx), func_line);
                let name_idx = self.name_constant(name);
                self.chunk.write(Op::DefineGlobal(name_idx), func_line);
            }
            Stmt::Return(value) => {
                let line = match value {
                    Some(e) => {
                        self.expression(e);
                        expr_line(e)
                    }
                    None => {
                        self.chunk.write(Op::Nil, 0); // line doesn't matter much for implicit return
                        0
                    }
                };
                self.chunk.write(Op::Return, line);
            }
            Stmt::Break(_) => {
                if self.loop_breaks.is_empty() {
                    // This should be caught by the parser, but just in case
                    panic!("Break statement outside of loop");
                }
                let line = 0; // line doesn't matter for jump
                let jump_pos = self.chunk.write(Op::Jump(0), line);
                // Record this break jump in the current (innermost) loop level
                let current_loop = self.loop_breaks.len() - 1;
                self.loop_breaks[current_loop].push(jump_pos);
            }
        }
    }

    // --- expressions ------------------------------------------------------------------

    /// Emits the code for `expr` and returns the line the caller should attribute its own
    /// follow-up instruction to.
    fn expression(&mut self, expr: &Expr) -> usize {
        let line = expr_line(expr);
        match expr {
            Expr::Literal { value, .. } => {
                match value {
                    Value::Nil => { self.chunk.write(Op::Nil, line); }
                    Value::Bool(true) => { self.chunk.write(Op::True, line); }
                    Value::Bool(false) => { self.chunk.write(Op::False, line); }
                    Value::Number(n) => {
                        let idx = self.chunk.constant(Value::Number(*n));
                        self.chunk.write(Op::Constant(idx), line);
                    }
                    Value::Str(s) => {
                        let idx = self.chunk.constant(Value::Str(s.clone()));
                        self.chunk.write(Op::Constant(idx), line);
                    }
                    Value::Function { .. } => {
                        // Functions are not literals in expressions
                        // This should not happen in valid Kobo programs
                    }
                }
            }
            Expr::Grouping(inner) => {
                self.expression(inner);
            }
            Expr::Variable { name, at } => {
                match *at {
                    Resolved::Global => {
                        let idx = self.name_constant(&name.lexeme);
                        self.chunk.write(Op::GetGlobal(idx), line);
                    }
                    Resolved::Local { slot, .. } => {
                        self.chunk.write(Op::GetLocal(slot), line);
                    }
                }
            }
            Expr::Assign { name, value, at } => {
                self.expression(value);
                match *at {
                    Resolved::Global => {
                        let idx = self.name_constant(&name.lexeme);
                        self.chunk.write(Op::SetGlobal(idx), line);
                    }
                    Resolved::Local { slot, .. } => {
                        self.chunk.write(Op::SetLocal(slot), line);
                    }
                }
            }
            Expr::Call { callee, paren, args } => {
                self.expression(callee);
                for arg in args {
                    self.expression(arg);
                }
                self.chunk.write(Op::Call(args.len()), paren.line);
            }
            Expr::Unary { op, right } => {
                self.expression(right);
                match op.kind {
                    TokenType::Minus => { self.chunk.write(Op::Negate, line); }
                    TokenType::Bang => { self.chunk.write(Op::Not, line); }
                    _ => {}
                }
            }
            Expr::Binary { left, op, right } => {
                self.expression(left);
                self.expression(right);
                match op.kind {
                    TokenType::Plus => { self.chunk.write(Op::Add, line); }
                    TokenType::Minus => { self.chunk.write(Op::Subtract, line); }
                    TokenType::Star => { self.chunk.write(Op::Multiply, line); }
                    TokenType::Slash => { self.chunk.write(Op::Divide, line); }
                    TokenType::Greater => { self.chunk.write(Op::Greater, line); }
                    TokenType::GreaterEqual => { self.chunk.write(Op::GreaterEqual, line); }
                    TokenType::Less => { self.chunk.write(Op::Less, line); }
                    TokenType::LessEqual => { self.chunk.write(Op::LessEqual, line); }
                    TokenType::EqualEqual => { self.chunk.write(Op::Equal, line); }
                    TokenType::BangEqual => { self.chunk.write(Op::NotEqual, line); }
                    _ => {}
                }
            }
            Expr::Logical { left, op, right } => {
                match op.kind {
                    TokenType::Or => {
                        self.expression(left);
                        // If left is truthy, jump over right
                        let jump = self.chunk.write(Op::JumpIfFalse(0), line);
                        self.chunk.write(Op::Pop, line); // Pop left value
                        self.expression(right);
                        self.patch(jump);
                    }
                    TokenType::And => {
                        self.expression(left);
                        // If left is falsey, jump over right
                        let jump = self.chunk.write(Op::JumpIfFalse(0), line);
                        self.chunk.write(Op::Pop, line); // Pop left value
                        self.expression(right);
                        self.patch(jump);
                    }
                    _ => {}
                }
            }
        }
        line
    }

    // --- names ------------------------------------------------------------------------

    /// At the top level a name is global and stays a name. Inside any scope it is a local,
    /// and the value already sitting on the stack *is* the variable; declaring it is
    /// nothing more than agreeing which slot it occupies.
    /// A global is written into the globals map; a local is already on the stack in the
    /// slot the resolver gave it, so declaring one emits nothing. `slot` is the
    /// resolver's answer and `in_function` is the one case it cannot express on its own:
    /// the script's outermost declarations are globals and a function body's are not.
    fn declare(&mut self, name: &str, slot: Option<usize>, line: usize) {
        if slot.is_none() {
            // Global
            let idx = self.name_constant(name);
            self.chunk.write(Op::DefineGlobal(idx), line);
        }
        // Local: already on stack in the correct slot, nothing to emit
    }

    fn name_constant(&mut self, name: &str) -> usize {
        self.chunk.constant(Value::Str(name.to_string()))
    }

    /// Leaving a scope pops its locals off the stack, one `Pop` each. How many is the
    /// resolver's count, carried on the `Block`; this file no longer tracks scopes to
    /// work it out a second time. The interpreter dropped a whole `HashMap` here; the VM
    /// adjusts a stack pointer.
    fn end_scope(&mut self, locals: usize) {
        for _ in 0..locals {
            self.chunk.write(Op::Pop, 0); // line doesn't matter for Pop
        }
    }

    fn patch(&mut self, at: usize) {
        let target = self.chunk.code.len();
        if let Op::JumpIfFalse(_) | Op::Jump(_) = self.chunk.code[at] {
            self.chunk.code[at] = match self.chunk.code[at] {
                Op::JumpIfFalse(_) => Op::JumpIfFalse(target),
                Op::Jump(_) => Op::Jump(target),
                _ => unreachable!(),
            };
        }
    }
}
