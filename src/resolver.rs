use crate::ast::{Expr, Function, Program, Resolved, Stmt};

/// Binds every name to its declaration once, before either engine runs. Scoping is
/// lexical (spec section 4.2), so the answer is a property of the text and cannot change
/// between runs: the interpreter gets a depth to index with, the VM a stack slot.
///
/// Globals are deliberately left unresolved: one may be used before its `var` has run,
/// which is what makes mutual recursion legal, so a name that matches no local is a
/// global by construction. Spec section 7.
pub fn resolve(program: &mut Program) {
    let mut resolver = Resolver::default();

    // Resolve each function body in its own frame
    for function in &mut program.functions {
        resolver.begin_function(&function.params);
        resolver.statements(&mut function.body);
        resolver.end_function();
    }

    // Resolve the script (global scope)
    resolver.depth = 0;
    resolver.locals.clear();
    resolver.statements(&mut program.stmts);
}

struct Local {
    name: String,
    scope: usize,
    /// False between `declare` and `define`, so a binding is invisible while its own
    /// initialiser is resolved and `var a = a + 1;` reads the enclosing `a` (section 4.2).
    initialised: bool,
}

/// Chapter 11 of *Crafting Interpreters* keys its scopes by name, `Vec<HashMap<String,
/// bool>>`. That cannot hold Kobo: section 4.2 licenses `{ var a = 1; var a = a + 1; }`, where
/// one scope has two live bindings of `a` and the second's initialiser reads the first.
/// A flat list holds both, carries the flag per binding, and gives the slot as its index.
#[derive(Default)]
struct Resolver {
    locals: Vec<Local>,
    depth: usize,
}

impl Resolver {
    /// Begin resolving a function body. Parameters are declared and defined in the function's scope.
    fn begin_function(&mut self, params: &[String]) {
        self.depth += 1; // Enter function scope
        // Declare and define parameters
        for param in params {
            self.declare(param);
            self.define(param);
        }
    }

    /// End resolving a function body.
    fn end_function(&mut self) {
        self.end_scope(); // Exit function scope
    }

    /// Returns how many locals the scope held: the number of `Pop`s the bytecode
    /// compiler owes for leaving it.
    fn end_scope(&mut self) -> usize {
        let mut count = 0;
        // Count locals at the current depth
        while let Some(local) = self.locals.last() {
            if local.scope == self.depth {
                self.locals.pop();
                count += 1;
            } else {
                break;
            }
        }
        self.depth -= 1;
        count
    }

    fn declare(&mut self, name: &str) {
        // Only track locals (depth > 0) in the flat list. Globals (depth == 0) are
        // resolved by name at runtime via the globals map, so they don't need slots.
        if self.depth > 0 {
            self.locals.push(Local {
                name: name.to_string(),
                scope: self.depth,
                initialised: false,
            });
        }
    }

    /// `None` at the top level of the script, where a declaration is a global.
    fn define(&mut self, name: &str) -> Option<usize> {
        if self.depth == 0 {
            // Global scope - no slot
            return None;
        }
        // Find the last declared local with this name at the current depth
        // and mark it as initialised
        for i in (0..self.locals.len()).rev() {
            if self.locals[i].name == name && self.locals[i].scope == self.depth {
                self.locals[i].initialised = true;
                return Some(i);
            }
        }
        None
    }

    /// Innermost first, so the nearest declaration wins and an outer one stays shadowed.
    fn lookup(&self, name: &str) -> Resolved {
        // Search from innermost to outermost
        for i in (0..self.locals.len()).rev() {
            let local = &self.locals[i];
            if local.name == name {
                if !local.initialised {
                    // Still initialising - this is an error in the language but resolver doesn't report errors
                    // The spec says resolution reports no errors, so we treat it as a global?
                    // Actually, the spec says the binding is invisible while its own initialiser is resolved
                    // So we should skip it and continue searching
                    continue;
                }
                // Calculate depth: how many scopes out from the innermost
                // The innermost scope is at depth = self.depth
                // Local's scope is the depth at which it was declared
                let depth = self.depth - local.scope;
                return Resolved::Local { depth, slot: i };
            }
        }
        // No local found - it's a global
        Resolved::Global
    }

    fn statements(&mut self, stmts: &mut [Stmt]) {
        for stmt in stmts {
            self.statement(stmt);
        }
    }

    fn statement(&mut self, stmt: &mut Stmt) {
        match stmt {
            Stmt::Expression(e) | Stmt::Print(e) => self.expression(e),

            Stmt::Var {
                name,
                initialiser,
                slot,
            } => {
                // Declare, resolve, define, in that order, which is the whole of section 4.2.
                self.declare(&name.lexeme);
                if let Some(e) = initialiser {
                    self.expression(e);
                }
                *slot = self.define(&name.lexeme);
            }

            Stmt::Block { body, locals } => {
                self.depth += 1;
                self.statements(body);
                *locals = self.end_scope();
            }

            Stmt::If {
                condition,
                then_branch,
                else_branch,
            } => {
                self.expression(condition);
                self.statement(then_branch);
                if let Some(alt) = else_branch {
                    self.statement(alt);
                }
            }

            Stmt::While { condition, body } => {
                self.expression(condition);
                self.statement(body);
            }

            // The body was resolved in its own frame above; only the name binds here.
            Stmt::Function { name, slot, .. } => {
                self.declare(name);
                *slot = self.define(name);
            }

            Stmt::Return(value) => {
                if let Some(e) = value {
                    self.expression(e);
                }
            }
            Stmt::Break(_) => {
                // Break has no expressions to resolve
            }
        }
    }

    fn expression(&mut self, expr: &mut Expr) {
        match expr {
            Expr::Literal { .. } => {}
            Expr::Grouping(inner) => self.expression(inner),

            Expr::Variable { name, at } => *at = self.lookup(&name.lexeme),

            Expr::Assign { name, value, at } => {
                self.expression(value);
                *at = self.lookup(&name.lexeme);
            }

            Expr::Call { callee, args, .. } => {
                self.expression(callee);
                for arg in args {
                    self.expression(arg);
                }
            }

            Expr::Unary { right, .. } => self.expression(right),

            Expr::Binary { left, right, .. } | Expr::Logical { left, right, .. } => {
                self.expression(left);
                self.expression(right);
            }
        }
    }
}
