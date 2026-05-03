use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::Result;
use notify::{Event, EventKind, RecursiveMode, Watcher};

use crate::CognitiveProjectLayer;
use crate::scanner::{IgnoreMatcher, is_text_candidate};

pub fn watch_project(root: impl AsRef<Path>, debounce: Duration) -> Result<()> {
    let root = root.as_ref().canonicalize()?;
    let (tx, rx) = mpsc::channel::<notify::Result<Event>>();
    let mut watcher = notify::recommended_watcher(move |event| {
        let _ = tx.send(event);
    })?;
    watcher.watch(&root, RecursiveMode::Recursive)?;

    println!("Cognitive Project Layer watcher");
    println!("Root: {}", root.display());
    println!("Debounce: {} ms", debounce.as_millis());
    println!("Press Ctrl+C to stop.");

    let mut layer = CognitiveProjectLayer::initialize(&root)?;
    let ignore_matcher = IgnoreMatcher::from_root(&root);
    println!("{}", layer.indexer.render_status());

    let mut pending = BTreeSet::<PathBuf>::new();
    let mut last_event = Instant::now();

    loop {
        match rx.recv_timeout(debounce) {
            Ok(Ok(event)) => {
                if is_relevant_event(&event) {
                    for path in event.paths {
                        if is_relevant_path(&root, &path, &ignore_matcher) {
                            pending.insert(path);
                        }
                    }
                    last_event = Instant::now();
                }
            }
            Ok(Err(error)) => eprintln!("watch error: {error}"),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if !pending.is_empty() && last_event.elapsed() >= debounce {
                    let paths = pending.iter().cloned().collect::<Vec<_>>();
                    pending.clear();
                    for path in paths {
                        if path.exists() {
                            match layer.on_file_save(&path) {
                                Ok(()) => println!("updated: {}", display_rel(&root, &path)),
                                Err(error) => {
                                    eprintln!("update failed for {}: {error}", path.display())
                                }
                            }
                        } else {
                            match CognitiveProjectLayer::initialize(&root) {
                                Ok(new_layer) => {
                                    layer = new_layer;
                                    println!(
                                        "reindexed after delete: {}",
                                        display_rel(&root, &path)
                                    );
                                }
                                Err(error) => eprintln!("reindex failed after delete: {error}"),
                            }
                        }
                    }
                    println!(
                        "state={:?}; graph_edges={}; chunks={}",
                        layer.indexer.state,
                        layer.graph.edges.len(),
                        layer.vector_store.len()
                    );
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    Ok(())
}

fn is_relevant_event(event: &Event) -> bool {
    matches!(
        event.kind,
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
    )
}

fn is_relevant_path(root: &Path, path: &Path, ignore_matcher: &IgnoreMatcher) -> bool {
    if ignore_matcher.should_ignore(root, path) {
        return false;
    }
    if let Ok(rel) = path.strip_prefix(root)
        && (ignore_matcher.should_ignore(root, rel) || rel.starts_with(".cpl"))
    {
        return false;
    }
    path.is_file() && is_text_candidate(path) || !path.exists()
}

fn display_rel(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string()
}
