# CA2 Reflection

## 1. Dangling `else` resolution

**Lines in `if_statement` that resolve the dangling `else`:** `src/parser.rs:155-171`

The `if_statement` function (lines 155-171) parses the `if` condition, then the `then_branch` by calling `self.statement()?`, and then checks for an optional `else` branch with `if self.matches(&[TokenType::Else])`. If `else` is present, it parses another statement as the `else_branch`.

**Why recursion decides it rather than the grammar:**

The grammar in section 3.1 is ambiguous:
```
ifStmt → "if" "(" expression ")" statement ( "else" statement )?
```

It doesn't specify which `if` an `else` belongs to when nested. However, recursive descent naturally resolves this because the inner `if_statement` call consumes the `else` before returning. When parsing `if (false) if (true) print "a"; else print "b";`:

1. Outer `if_statement` parses condition `false`
2. Parses then_branch by calling `statement()`, which calls `if_statement` again
3. Inner `if_statement` parses condition `true`
4. Parses then_branch: `print "a";`
5. Sees `else`, consumes it, parses else_branch: `print "b";`
6. Inner `if_statement` returns with `else_branch = Some(print "b")`
7. Outer `if_statement` sees no `else` (already consumed), so its `else_branch = None`

**What the parser prints for `if (false) if (true) print "a"; else print "b";`:**
```
(if false (if true (print "a") (print "b")))
```

**What it would print if `else` bound to outer `if`:**
```
(if false (if true (print "a")) (print "b"))
```

The current behavior binds the `else` to the nearest (inner) `if`, which is the standard behavior in most languages and falls out of recursive descent for free.

---

## 2. Assignment left-side check

**Line in `assignment` where left side is checked:** `src/parser.rs:219`

```rust
if let Expr::Variable { name, .. } = &expr {
```

**Why this check cannot happen before the left side is parsed:**

The assignment rule in section 3.2 is:
```
assignment → IDENTIFIER "=" assignment | logic_or
```

This is not LL(1) because when the parser sees an identifier (`a`), it cannot know with only one token of lookahead whether it's:
- A variable reference (part of `logic_or` → `or` → `and` → ... → `primary`)
- The left side of an assignment (`a = ...`)

Both paths start identically. The parser must parse the left side as a general expression first (which could be `a`, `a.b`, `f()`, etc.), then check if the next token is `=`. If it is, the left side must be a bare identifier (`Variable` node); otherwise it's a syntax error.

**Message from section 5.1 that `1 = 2;` produces:**
```
[line 1] Error at '=': Cannot assign to this expression.
```

---

## 3. Synchronisation tokens

**Function `synchronise` location:** `src/parser.rs:430-460`

**Tokens the synchroniser stops on (when positioned at them):**
- `fun` — starts a function declaration
- `var` — starts a variable declaration  
- `if` — starts an if statement
- `while` — starts a while statement
- `print` — starts a print statement
- `return` — starts a return statement

**Also stops after consuming:** `;` (semicolon)

**Also consumes (to exit broken scopes):** `}` (RBrace) and `)` (RParen)

**Why these tokens:** They are the only tokens that can legally begin a declaration or statement at the top level or block level (section 3.1). By synchronizing to these points, the parser recovers from an error and resumes parsing at the next valid statement boundary, preventing one error from cascading into many.

**Test whose output changes if `synchronise` call is deleted:** `tests/phase-2/invalid/expect_semicolon_after_value.kobo`

**Input:**
```kobo
print 1
print 2;
```

**Current output (with synchronise):**
```
[line 2] Error at 'print': Expect ';' after value.
(print 2)
```

**Output without synchronise (call deleted):**
The parser would not recover after the missing semicolon on line 1. It would continue parsing `print 2;` as part of the same broken expression, likely producing multiple errors or a completely different AST, and the second `print 2;` would not be parsed as a valid statement. The single error would cascade into many.

---

## 4. Commit history anchors

**Commit where `if_statement` dangling else was wrong:** `d6fb308`
```rust
    fn if_statement(&mut self) -> Parsed<Stmt> {
        // TODO(you): spec 3.1. The dangling else binds to the nearest unmatched if , the
        //            recursion decides that, not the grammar.
        todo!("if_statement")
    }
```

**Commit where `if_statement` was fixed:** `48aa122`
```rust
    fn if_statement(&mut self) -> Parsed<Stmt> {
        self.consume(TokenType::LParen, "Expect '(' after 'if'.")?;
        let condition = self.expression()?;
        self.consume(TokenType::RParen, "Expect ')' after if condition.")?;

        let then_branch = Box::new(self.statement()?);
        let else_branch = if self.matches(&[TokenType::Else]) {
            Some(Box::new(self.statement()?))
        } else {
            None
        };

        Ok(Stmt::If {
            condition,
            then_branch,
            else_branch,
        })
    }
```

**Commit where `assignment` left-side check was wrong:** `d6fb308`
```rust
    fn assignment(&mut self) -> Parsed<Expr> {
        // TODO(you): spec 3.2. The left side parses as an ordinary expression and is checked
        //            afterwards, because the rule is not LL(1).
        // TEMPORARY: enough of this rule to let the ones below it run. Replace it in its
        //            own week; the marking tests for it fail until you do.
        self.or()
    }
```

**Commit where `assignment` check was fixed:** `48aa122`
```rust
    fn assignment(&mut self) -> Parsed<Expr> {
        let expr = self.or()?;

        if self.matches(&[TokenType::Equal]) {
            let value = self.assignment()?;
            if let Expr::Variable { name, .. } = &expr {
                return Ok(Expr::Assign {
                    name: name.clone(),
                    value: Box::new(value),
                    at: Resolved::Global,
                });
            }
            self.error("Cannot assign to this expression.");
            return Err(());
        }

        Ok(expr)
    }
```

**Commit where `synchronise` was wrong:** `d6fb308`
```rust
    fn synchronise(&mut self) {
        // TODO(you): spec 5.2: after an error, discard tokens until just past a ';' or on a
        //            statement keyword, so one mistake costs one message.
        // TEMPORARY: enough of this rule to let the ones below it run. Replace it in its
        //            own week; the marking tests for it fail until you do.
        self.depth = 0;
        while !self.at_end() {
            self.current += 1;
            if self.previous().kind == TokenType::Semicolon {
                return;
            }
        }
    }
```

**Commit where `synchronise` was fixed:** `48aa122`
```rust
    fn synchronise(&mut self) {
        self.depth = 0;
        while !self.at_end() {
            if self.previous().kind == TokenType::Semicolon {
                // If we stopped after a semicolon but the next token is a scope ender,
                // consume it to pop out of the broken scope and avoid infinite loops.
                if matches!(
                    self.peek().kind,
                    TokenType::RBrace | TokenType::RParen
                ) {
                    self.current += 1;
                }
                return;
            }
            match self.peek().kind {
                TokenType::Fun
                | TokenType::Var
                | TokenType::If
                | TokenType::While
                | TokenType::Print
                | TokenType::Return => return,
                TokenType::RBrace | TokenType::RParen => {
                    // Consume scope end markers to pop out of broken scopes
                    self.current += 1;
                }
                _ => {
                    self.current += 1;
                }
            }
        }
    }
```

*Both commits are your own and at or before the deadline. Use `git log -p --follow src/parser.rs` to verify.*