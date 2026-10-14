The crash comes from `Session::resume` reading the journal before the lock is held. Three changes fix it:

1. Take the lock first, in `open_journal`.
2. Read the header **after** the lock, so a concurrent writer cannot truncate it.
3. Return `Err(Locked)` instead of panicking when another process holds it.

```rust
pub fn open_journal(path: &Path) -> Result<Journal, OpenError> {
    let lock = FileLock::acquire(path.with_extension("lock"))?;
    let header = Header::read(path)?;
    Ok(Journal { lock, header })
}
```

After that, `cargo test journal` passes and the resume path in `src/session.rs:212` no longer races. See [the locking notes](https://example.com/docs/locking) for why an advisory lock is enough here.
