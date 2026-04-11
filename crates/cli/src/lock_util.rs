use std::sync::{MutexGuard, PoisonError, RwLockReadGuard, RwLockWriteGuard};

/// Extension trait for recovering from poisoned locks instead of
/// panicking.  When a thread panics while holding a lock, the lock
/// becomes poisoned and all subsequent `.unwrap()` calls cascade
/// into more panics.  `unwrap_or_recover` logs a warning and
/// recovers via `PoisonError::into_inner`, keeping the daemon alive.
pub trait RecoverPoison<T> {
  fn unwrap_or_recover(self) -> T;
}

impl<'a, T> RecoverPoison<RwLockReadGuard<'a, T>>
  for Result<RwLockReadGuard<'a, T>, PoisonError<RwLockReadGuard<'a, T>>>
{
  fn unwrap_or_recover(self) -> RwLockReadGuard<'a, T> {
    match self {
      Ok(guard) => guard,
      Err(poisoned) => {
        tracing::warn!("RwLock read guard recovered from poison");
        poisoned.into_inner()
      }
    }
  }
}

impl<'a, T> RecoverPoison<RwLockWriteGuard<'a, T>>
  for Result<RwLockWriteGuard<'a, T>, PoisonError<RwLockWriteGuard<'a, T>>>
{
  fn unwrap_or_recover(self) -> RwLockWriteGuard<'a, T> {
    match self {
      Ok(guard) => guard,
      Err(poisoned) => {
        tracing::warn!("RwLock write guard recovered from poison");
        poisoned.into_inner()
      }
    }
  }
}

impl<'a, T> RecoverPoison<MutexGuard<'a, T>>
  for Result<MutexGuard<'a, T>, PoisonError<MutexGuard<'a, T>>>
{
  fn unwrap_or_recover(self) -> MutexGuard<'a, T> {
    match self {
      Ok(guard) => guard,
      Err(poisoned) => {
        tracing::warn!("Mutex guard recovered from poison");
        poisoned.into_inner()
      }
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::sync::{Arc, Mutex, RwLock};

  #[test]
  fn recover_from_poisoned_rwlock_read() {
    let lock = Arc::new(RwLock::new(42));
    let lock2 = Arc::clone(&lock);
    let _ = std::thread::spawn(move || {
      let _guard = lock2.write().unwrap();
      panic!("intentional poison");
    })
    .join();

    // The lock is now poisoned.
    assert!(lock.read().is_err());
    let guard = lock.read().unwrap_or_recover();
    assert_eq!(*guard, 42);
  }

  #[test]
  fn recover_from_poisoned_rwlock_write() {
    let lock = Arc::new(RwLock::new(42));
    let lock2 = Arc::clone(&lock);
    let _ = std::thread::spawn(move || {
      let _guard = lock2.write().unwrap();
      panic!("intentional poison");
    })
    .join();

    assert!(lock.write().is_err());
    let mut guard = lock.write().unwrap_or_recover();
    *guard = 99;
    assert_eq!(*guard, 99);
  }

  #[test]
  fn recover_from_poisoned_mutex() {
    let lock = Arc::new(Mutex::new(42));
    let lock2 = Arc::clone(&lock);
    let _ = std::thread::spawn(move || {
      let _guard = lock2.lock().unwrap();
      panic!("intentional poison");
    })
    .join();

    assert!(lock.lock().is_err());
    let guard = lock.lock().unwrap_or_recover();
    assert_eq!(*guard, 42);
  }

  #[test]
  fn healthy_lock_returns_guard_normally() {
    let rw = RwLock::new(10);
    let guard = rw.read().unwrap_or_recover();
    assert_eq!(*guard, 10);
    drop(guard);

    let mut guard = rw.write().unwrap_or_recover();
    *guard = 20;
    assert_eq!(*guard, 20);
    drop(guard);

    let mtx = Mutex::new(30);
    let guard = mtx.lock().unwrap_or_recover();
    assert_eq!(*guard, 30);
  }
}
