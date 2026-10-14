## Review: sidebar ordering

Overall this is *close*, but two things need another pass.

- **Ordering** — rows still re-sort when a session finishes:
  - `rebuild_sidebar_sessions` sorts by `updated_at`;
  - the user's drag order is ignored after a restart.
- **Naming** — `row_ix` reads as a typo; prefer `row_index`.

> The sidebar order is the user's; the app should never move a row on its own.

| File | Lines | Verdict |
| :--- | ---: | :---: |
| `src/sidebar.rs` | 42 | changes needed |
| `src/main.rs` | 7 | fine |

```diff
-    rows.sort_by_key(|row| row.updated_at);
+    rows.sort_by_key(|row| row.position);
```

Happy to look again once the order is stable. ✅
