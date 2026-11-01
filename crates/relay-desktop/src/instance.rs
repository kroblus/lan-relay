//! Single instance scoped to the selected user's data directory. The only IPC
//! command is a filesystem wake-up, never an unauthenticated LAN control API.
use fs2::FileExt;
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
};

pub enum Instance {
    Primary(InstanceGuard),
    Existing,
}

pub struct InstanceGuard {
    _lock: File,
    _watcher: Option<RecommendedWatcher>,
    wake_path: PathBuf,
}

pub fn acquire(directory: &Path) -> io::Result<Instance> {
    fs::create_dir_all(directory)?;
    // File notifications carry absolute paths. Canonicalizing also keeps an
    // explicit relative --data-dir equivalent to its absolute spelling.
    let directory = fs::canonicalize(directory)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
    }
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(directory.join("desktop-instance.lock"))?;
    let wake_path = directory.join("desktop-show.request");
    match lock.try_lock_exclusive() {
        Ok(()) => Ok(Instance::Primary(InstanceGuard {
            _lock: lock,
            _watcher: None,
            wake_path,
        })),
        Err(error) if error.raw_os_error() == fs2::lock_contended_error().raw_os_error() => {
            // Windows reports ERROR_LOCK_VIOLATION, which is not necessarily
            // classified as WouldBlock by std::io. Use fs2's platform contract.
            // A fixed literal is intentionally the entire IPC vocabulary. No
            // path, token, or shell command is accepted from this file.
            let mut wake = OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(wake_path)?;
            wake.write_all(b"show\n")?;
            Ok(Instance::Existing)
        }
        Err(error) => Err(error),
    }
}

impl InstanceGuard {
    pub fn watch(&mut self, callback: impl Fn() + Send + 'static) -> Result<(), notify::Error> {
        let wake_path = self.wake_path.clone();
        let directory = wake_path
            .parent()
            .expect("wake path has a parent")
            .to_owned();
        // Ensure a wake arriving immediately after the first lock acquisition
        // is delivered after the watcher has attached.
        let existed = wake_path.exists();
        let mut watcher =
            notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                if let Ok(event) = event {
                    if event.paths.iter().any(|p| p == &wake_path) && !event.kind.is_remove() {
                        callback();
                    }
                }
            })?;
        watcher.watch(&directory, RecursiveMode::NonRecursive)?;
        self._watcher = Some(watcher);
        if existed {
            // Produces a watcher event for a request queued during startup.
            let _ = fs::write(&self.wake_path, b"show\n");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lock_prevents_a_second_host_and_wakes_the_first() {
        let current = std::env::current_dir().unwrap();
        let dir = tempfile::tempdir_in(&current).unwrap();
        let relative = dir.path().strip_prefix(&current).unwrap();
        let Instance::Primary(mut first) = acquire(relative).unwrap() else {
            panic!("first instance must own its data directory");
        };
        let (sender, receiver) = std::sync::mpsc::channel();
        first
            .watch(move || {
                let _ = sender.send(());
            })
            .unwrap();
        assert!(matches!(acquire(dir.path()).unwrap(), Instance::Existing));
        receiver
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        assert_eq!(
            fs::read(dir.path().join("desktop-show.request")).unwrap(),
            b"show\n"
        );
        drop(first);
        assert!(matches!(acquire(dir.path()).unwrap(), Instance::Primary(_)));
    }
}
