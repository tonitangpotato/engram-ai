//! File-based maintenance lock for multi-process safety.
//!
//! Leader election via lock file containing PID + timestamp.
//! Stale lock recovery when the holding PID is no longer running.

use std::fs;
use std::io::Write;
use std::path::PathBuf;

use chrono::{DateTime, Utc};

use super::types::KcError;

/// Lock status.
#[derive(Debug, Clone)]
pub enum LockStatus {
    /// No lock file exists.
    Free,
    /// Lock held by a running process.
    Held { pid: u32, since: DateTime<Utc> },
    /// Lock file exists but PID is not running.
    Stale { pid: u32, since: DateTime<Utc> },
}

/// RAII guard that releases the lock on drop.
pub struct MaintenanceGuard {
    lock_path: PathBuf,
}

impl Drop for MaintenanceGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.lock_path);
    }
}

/// File-based maintenance lock.
pub struct MaintenanceLock {
    lock_path: PathBuf,
}

impl MaintenanceLock {
    /// Create a lock targeting the given directory.
    /// Lock file: `{dir}/.engram-maintenance.lock`
    pub fn new(dir: &std::path::Path) -> Self {
        Self {
            lock_path: dir.join(".engram-maintenance.lock"),
        }
    }

    /// Try to acquire exclusive lock.
    /// If lock is stale (PID not running), force-acquire with warning log.
    pub fn try_acquire(&self) -> Result<MaintenanceGuard, KcError> {
        match self.status() {
            LockStatus::Free => self.write_lock(),
            LockStatus::Stale { pid, .. } => {
                log::warn!(
                    "Stale maintenance lock (PID {} no longer running), force-acquiring",
                    pid
                );
                self.write_lock()
            }
            LockStatus::Held { pid, since } => Err(KcError::Storage(format!(
                "Maintenance lock held by PID {} since {}",
                pid, since
            ))),
        }
    }

    /// Check current lock status.
    pub fn status(&self) -> LockStatus {
        let content = match fs::read_to_string(&self.lock_path) {
            Ok(c) => c,
            Err(_) => return LockStatus::Free,
        };

        // Lock file format: "PID\nTIMESTAMP_RFC3339"
        let mut lines = content.lines();
        let pid: u32 = match lines.next().and_then(|s| s.parse().ok()) {
            Some(p) => p,
            None => return LockStatus::Free, // Corrupt lock file
        };
        let since: DateTime<Utc> =
            match lines.next().and_then(|s| DateTime::parse_from_rfc3339(s).ok()) {
                Some(dt) => dt.with_timezone(&Utc),
                None => Utc::now(), // Missing timestamp, use now
            };

        if Self::is_pid_running(pid) {
            LockStatus::Held { pid, since }
        } else {
            LockStatus::Stale { pid, since }
        }
    }

    /// Write lock file with current PID and timestamp.
    fn write_lock(&self) -> Result<MaintenanceGuard, KcError> {
        let pid = std::process::id();
        let now = Utc::now().to_rfc3339();
        let content = format!("{}\n{}", pid, now);

        // Ensure parent directory exists
        if let Some(parent) = self.lock_path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| KcError::Storage(format!("Cannot create lock directory: {}", e)))?;
        }

        let mut file = fs::File::create(&self.lock_path)
            .map_err(|e| KcError::Storage(format!("Cannot create lock file: {}", e)))?;
        file.write_all(content.as_bytes())
            .map_err(|e| KcError::Storage(format!("Cannot write lock file: {}", e)))?;

        Ok(MaintenanceGuard {
            lock_path: self.lock_path.clone(),
        })
    }

    /// Check if a PID is still running.
    ///
    /// Uses `kill -0` which checks process existence without sending a signal.
    /// Falls back to assuming the process is running if the check itself fails.
    fn is_pid_running(pid: u32) -> bool {
        std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(true) // If we can't check, assume running (conservative)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lock_acquire_and_release() {
        let dir = tempfile::tempdir().unwrap();
        let lock = MaintenanceLock::new(dir.path());

        // Initially free
        assert!(matches!(lock.status(), LockStatus::Free));

        // Acquire
        let guard = lock.try_acquire().unwrap();

        // Should be held by our PID
        match lock.status() {
            LockStatus::Held { pid, .. } => assert_eq!(pid, std::process::id()),
            other => panic!("Expected Held, got {:?}", other),
        }

        // Drop guard → lock released
        drop(guard);
        assert!(matches!(lock.status(), LockStatus::Free));
    }

    #[test]
    fn test_double_acquire_fails() {
        let dir = tempfile::tempdir().unwrap();
        let lock = MaintenanceLock::new(dir.path());

        let _guard = lock.try_acquire().unwrap();

        // Second acquire should fail because our PID is still running
        let result = lock.try_acquire();
        assert!(result.is_err());

        if let Err(KcError::Storage(msg)) = result {
            assert!(msg.contains("Maintenance lock held by PID"));
        } else {
            panic!("Expected KcError::Storage");
        }
    }

    #[test]
    fn test_stale_lock_recovery() {
        let dir = tempfile::tempdir().unwrap();
        let lock = MaintenanceLock::new(dir.path());

        // Write a lock file with a non-existent PID (99999999)
        let fake_pid = 99999999u32;
        let content = format!("{}\n{}", fake_pid, Utc::now().to_rfc3339());
        fs::write(dir.path().join(".engram-maintenance.lock"), content).unwrap();

        // Should be detected as stale
        match lock.status() {
            LockStatus::Stale { pid, .. } => assert_eq!(pid, fake_pid),
            other => panic!("Expected Stale, got {:?}", other),
        }

        // Acquire should succeed (force-acquires stale lock)
        let guard = lock.try_acquire().unwrap();

        // Now held by us
        match lock.status() {
            LockStatus::Held { pid, .. } => assert_eq!(pid, std::process::id()),
            other => panic!("Expected Held, got {:?}", other),
        }

        drop(guard);
    }

    #[test]
    fn test_corrupt_lock_file_treated_as_free() {
        let dir = tempfile::tempdir().unwrap();
        let lock = MaintenanceLock::new(dir.path());

        // Write garbage to lock file
        fs::write(dir.path().join(".engram-maintenance.lock"), "not-a-pid\ngarbage").unwrap();

        // Should treat corrupt file as free
        assert!(matches!(lock.status(), LockStatus::Free));

        // Should be able to acquire
        let _guard = lock.try_acquire().unwrap();
    }

    #[test]
    fn test_lock_file_empty_treated_as_free() {
        let dir = tempfile::tempdir().unwrap();
        let lock = MaintenanceLock::new(dir.path());

        // Empty lock file
        fs::write(dir.path().join(".engram-maintenance.lock"), "").unwrap();

        assert!(matches!(lock.status(), LockStatus::Free));
        let _guard = lock.try_acquire().unwrap();
    }

    #[test]
    fn test_lock_no_directory_error() {
        let lock = MaintenanceLock::new(std::path::Path::new("/nonexistent/path/xyz"));
        // Trying to acquire in non-existent dir should error
        let result = lock.try_acquire();
        assert!(result.is_err());
    }

    #[test]
    fn test_guard_drop_removes_file() {
        let dir = tempfile::tempdir().unwrap();
        let lock_path = dir.path().join(".engram-maintenance.lock");
        let lock = MaintenanceLock::new(dir.path());

        let guard = lock.try_acquire().unwrap();
        assert!(lock_path.exists());

        drop(guard);
        assert!(!lock_path.exists());
    }
}
