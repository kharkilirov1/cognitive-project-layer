use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::CognitiveProjectLayer;

#[derive(Debug, Clone)]
pub enum BackgroundCommand {
    RefreshFile(PathBuf),
    Reindex,
    Shutdown,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BackgroundStatus {
    pub refresh_jobs: usize,
    pub reindex_jobs: usize,
    pub last_error: Option<String>,
    pub running: bool,
}

pub struct BackgroundProjectLayer {
    sender: Sender<BackgroundCommand>,
    status: Arc<Mutex<BackgroundStatus>>,
    handle: Option<JoinHandle<()>>,
}

impl BackgroundProjectLayer {
    pub fn start(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref().canonicalize()?;
        let (sender, receiver) = mpsc::channel::<BackgroundCommand>();
        let status = Arc::new(Mutex::new(BackgroundStatus {
            running: true,
            ..BackgroundStatus::default()
        }));
        let worker_status = Arc::clone(&status);
        let handle = thread::spawn(move || {
            let mut layer = match CognitiveProjectLayer::initialize(&root) {
                Ok(layer) => layer,
                Err(error) => {
                    set_error(&worker_status, error);
                    set_running(&worker_status, false);
                    return;
                }
            };

            while let Ok(command) = receiver.recv() {
                match command {
                    BackgroundCommand::RefreshFile(path) => {
                        if let Err(error) = layer.on_file_save(&path) {
                            set_error(&worker_status, error);
                        } else {
                            let mut status = worker_status.lock().unwrap();
                            status.refresh_jobs += 1;
                            status.last_error = None;
                        }
                    }
                    BackgroundCommand::Reindex => match CognitiveProjectLayer::initialize(&root) {
                        Ok(new_layer) => {
                            layer = new_layer;
                            let mut status = worker_status.lock().unwrap();
                            status.reindex_jobs += 1;
                            status.last_error = None;
                        }
                        Err(error) => set_error(&worker_status, error),
                    },
                    BackgroundCommand::Shutdown => break,
                }
            }
            set_running(&worker_status, false);
        });

        Ok(Self {
            sender,
            status,
            handle: Some(handle),
        })
    }

    pub fn refresh_file(&self, path: impl Into<PathBuf>) -> Result<()> {
        self.sender
            .send(BackgroundCommand::RefreshFile(path.into()))?;
        Ok(())
    }

    pub fn reindex(&self) -> Result<()> {
        self.sender.send(BackgroundCommand::Reindex)?;
        Ok(())
    }

    pub fn status(&self) -> BackgroundStatus {
        self.status.lock().unwrap().clone()
    }

    pub fn shutdown(mut self) -> Result<BackgroundStatus> {
        let _ = self.sender.send(BackgroundCommand::Shutdown);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
        Ok(self.status())
    }
}

impl Drop for BackgroundProjectLayer {
    fn drop(&mut self) {
        let _ = self.sender.send(BackgroundCommand::Shutdown);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn set_error(status: &Arc<Mutex<BackgroundStatus>>, error: anyhow::Error) {
    let mut status = status.lock().unwrap();
    status.last_error = Some(error.to_string());
}

fn set_running(status: &Arc<Mutex<BackgroundStatus>>, running: bool) {
    status.lock().unwrap().running = running;
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use super::*;

    #[test]
    fn background_worker_accepts_refresh_and_shutdown() {
        let root = temp_project("background_worker");
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(
            root.join("Cargo.toml"),
            "[package]\nname='tmp'\nversion='0.1.0'\nedition='2024'\n",
        )
        .unwrap();
        let file = root.join("src/lib.rs");
        fs::write(&file, "pub fn old_name() {}\n").unwrap();

        let worker = BackgroundProjectLayer::start(&root).unwrap();
        fs::write(&file, "pub fn new_name() {}\n").unwrap();
        worker.refresh_file(file).unwrap();
        wait_for(|| worker.status().refresh_jobs >= 1 || worker.status().last_error.is_some());
        let status = worker.shutdown().unwrap();
        assert!(status.refresh_jobs >= 1);
        assert!(status.last_error.is_none());
        let _ = fs::remove_dir_all(root);
    }

    fn wait_for(mut predicate: impl FnMut() -> bool) {
        for _ in 0..50 {
            if predicate() {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn temp_project(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("cpl-{name}-{}", unique_suffix()))
    }

    fn unique_suffix() -> String {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        format!("{}-{nanos}", std::process::id())
    }
}
