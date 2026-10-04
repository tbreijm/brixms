use brix_kb::world::session::WorldLockGuard;

#[test]
fn lock_owner_child() {
    let Some(path) = std::env::var_os("BRIX_LOCK_TEST_DIRECTORY") else {
        return;
    };
    let _guard = WorldLockGuard::acquire(std::path::Path::new(&path)).unwrap();
    // Exiting without Rust destructors models abrupt loss of the writer.
    std::process::exit(0);
}

#[test]
fn writer_lock_excludes_concurrent_handles_and_recovers_after_process_exit() {
    let dir = std::env::temp_dir().join(format!("brix-writer-lock-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    {
        let _owner = WorldLockGuard::acquire(&dir).unwrap();
        assert!(WorldLockGuard::acquire(&dir).is_err());
    }
    let result = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "lock_owner_child"])
        .env("BRIX_LOCK_TEST_DIRECTORY", &dir)
        .status()
        .unwrap();
    assert!(result.success());
    let recovered = WorldLockGuard::acquire(&dir).unwrap();
    drop(recovered);
    std::fs::remove_dir_all(dir).unwrap();
}
