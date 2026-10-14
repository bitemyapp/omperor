Replies now wrap by **display width** instead of by byte count, so a line never splits a character and wide text lines up with the rest.

| File | Change |
| :--- | :--- |
| `src/measure.rs` | New: `wrap(text, width)` breaks at spaces and counts columns with `unicode-width` |
| `src/chat.rs` | `print_wrapped` prints the lines `measure::wrap` returns |
| `src/main.rs` | Declares the new module |
| `src/session.rs` | `recent(n)` returns the last *n* messages for the model's context |

The heart of it:

```rust src/measure.rs
let needed = if line.is_empty() {
    word.width()
} else {
    line.width() + 1 + word.width()
};
if needed > width && !line.is_empty() {
    lines.push(std::mem::take(&mut line));
}
```

The first test run failed because `main.rs` did not declare `mod measure`; with that fixed, `cargo test` passes, including a new case for CJK text:

```diff
-    for chunk in text.as_bytes().chunks(WIDTH) {
-        let _ = writeln!(out, "{}", String::from_utf8_lossy(chunk));
+    for line in measure::wrap(text, WIDTH) {
+        let _ = writeln!(out, "{line}");
     }
```

> [!NOTE]
> A single word longer than the width still gets a line of its own rather than being split. Say if you would rather hyphenate it.
