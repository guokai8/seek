use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

/// A check of the lock behaviour that the engine relies on.
///
/// The watcher polls the filesystem every few hundred milliseconds and
/// takes the write lock each time. The searches take the read lock. The
/// standard library's reader-writer lock prefers writers, so a stream of
/// writers can keep the readers waiting for a long time.
fn main() {
    let lock = Arc::new(RwLock::new(0u64));

    // The watcher: a writer every 400 ms.
    let writer_lock = Arc::clone(&lock);
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let writer_stop = Arc::clone(&stop);

    let writer = std::thread::spawn(move || {
        while !writer_stop.load(std::sync::atomic::Ordering::Relaxed) {
            {
                let mut guard = writer_lock.write().unwrap();
                *guard += 1;
                // The corpus rebuild, which takes about 60 ms.
                std::thread::sleep(Duration::from_millis(60));
            }
            std::thread::sleep(Duration::from_millis(340));
        }
    });

    // The search: a reader.
    let start = Instant::now();
    let mut waits = Vec::new();

    for _ in 0..5 {
        let search_start = Instant::now();
        let guard = lock.read().unwrap();
        waits.push(search_start.elapsed());
        std::thread::sleep(Duration::from_millis(5));
        drop(guard);
    }

    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    writer.join().unwrap();

    println!("reader waits: {waits:?}");
    println!(
        "total: {:?} (a search should take about 5ms)",
        start.elapsed()
    );
}
