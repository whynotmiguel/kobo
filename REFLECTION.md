# Reflection

## 1. Fractional part decision
The scanner decides that a '.' begins a fractional part only when it is followed by a digit. This is implemented at **src/scanner.rs:133** (`if self.peek() == '.' && self.peek_next().is_ascii_digit() {`). For the input `5.` the scanner consumes the '5' as a digit, sees the '.' but notes that the next character is not a digit (it is either EOF or whitespace), so it does **not** consume the dot. It then emits a `NUMBER` token with lexeme `"5"` and leaves the '.' in the input stream. The '.' will later be recognized as a stray character and trigger the error `[line N] Error: Character is not part of any token.` This matches **section 1.4** of the language spec, which states that a number is “one or more digits, optionally followed by `.` and one or more digits” and explicitly notes that `.` alone “does not scan as anything” and should cause the scanner to report that error.

## 2. Line‑counter changes and EOF line
The line counter is modified in two places:
- **src/scanner.rs:111** inside `string()` when a newline is encountered within a string literal (`if self.peek() == '\n' { self.line += 1; }`).
- **src/scanner.rs:222** inside `skip_whitespace()` when a newline is skipped between tokens (`if self.peek() == '\n' { self.line += 1; }`).

For a file that ends in two blank lines after the last real token, `skip_whitespace()` consumes both newlines, incrementing the line counter twice, but then detects `at_end()` and breaks out of the main loop before scanning another token. The `EOF` token is added via `add_eof()`, which uses `self.last_token_line` — the line of the last token that was actually added (recorded at **src/scanner.rs:164** each time `add()` is called). Therefore the `EOF` token carries the line of the **last real token**, not the line of the file’s final newline. Section 6.1 requires this because “a trailing newline left behind by an editor must not move a line number”; the EOF line should reflect the position of the last meaningful token, not irrelevant trailing whitespace.

## 3. Test failure and fix
I initially failed the test **tests/phase-1/invalid/unterminated_string.kobo**. The problem was that the scanner reported the *“String is never closed.”* error at the line where EOF was reached, not at the line where the string started.  
- **Wrong commit**: `ba73cb9` (Buggy: string error reports at EOF line instead of start line). At this commit, the relevant line was **src/scanner.rs:118**: `self.error(self.line, "String is never closed.");`.  
- **Fixed commit**: `d111181` (Fix: string error reports at start line, not EOF line). At this commit, I added **src/scanner.rs:108**: `let start_line = self.line;` to record the opening‑quote line, and changed **src/scanner.rs:118** to `self.error(start_line, "String is never closed.");`.  

My misunderstanding was that I thought the error should be reported where the scanner detected the missing closing quote (i.e., at EOF). The spec, however, explicitly states in section 1.5: “An unterminated string is an error reported at the line where the string *opened*.” Recording the starting line fixed the issue, and the test now passes.

---  
*All line numbers refer to src/scanner.rs in the commit referenced.*