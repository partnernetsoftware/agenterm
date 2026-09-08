use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::thread;
use std::time::{Duration, Instant};

const WORK: Duration = Duration::from_millis(250);
const CANCEL_AT: Duration = Duration::from_millis(25);

fn main() {
    let sync_elapsed = synchronous();
    let (detached_elapsed, detached_active, detached_lock_free) = detached();
    let (cooperative_elapsed, cooperative_active, cooperative_lock_free) = cooperative();
    println!(
        "synchronous_ms={} detached_ms={} detached_active={} detached_lock_free={} cooperative_ms={} cooperative_active={} cooperative_lock_free={}",
        sync_elapsed.as_millis(),
        detached_elapsed.as_millis(),
        detached_active,
        detached_lock_free,
        cooperative_elapsed.as_millis(),
        cooperative_active,
        cooperative_lock_free
    );
    assert!(sync_elapsed >= Duration::from_millis(200));
    assert!(detached_elapsed < Duration::from_millis(100));
    assert_eq!(detached_active, 1);
    assert!(!detached_lock_free);
    assert!(cooperative_elapsed < Duration::from_millis(100));
    assert_eq!(cooperative_active, 0);
    assert!(cooperative_lock_free);
}
fn cancel_later(flag: Arc<AtomicBool>) {
    thread::spawn(move || {
        thread::sleep(CANCEL_AT);
        flag.store(true, Ordering::Release);
    });
}

fn synchronous() -> Duration {
    let cancel = Arc::new(AtomicBool::new(false));
    cancel_later(Arc::clone(&cancel));
    let started = Instant::now();
    thread::sleep(WORK);
    assert!(cancel.load(Ordering::Acquire));
    started.elapsed()
}

fn detached() -> (Duration, usize, bool) {
    let cancel = Arc::new(AtomicBool::new(false));
    let active = Arc::new(AtomicUsize::new(0));
    let provider = Arc::new(Mutex::new(()));
    cancel_later(Arc::clone(&cancel));
    let active_worker = Arc::clone(&active);
    let provider_worker = Arc::clone(&provider);
    let handle = thread::spawn(move || {
        let _guard = provider_worker.lock().expect("provider lock");
        active_worker.fetch_add(1, Ordering::AcqRel);
        thread::sleep(WORK);
        active_worker.fetch_sub(1, Ordering::AcqRel);
    });
    while active.load(Ordering::Acquire) == 0 { thread::yield_now(); }
    let started = Instant::now();
    while !cancel.load(Ordering::Acquire) { thread::sleep(Duration::from_millis(2)); }
    let elapsed = started.elapsed();
    let observed_active = active.load(Ordering::Acquire);
    let lock_free = provider.try_lock().is_ok();
    handle.join().expect("detached probe worker");
    (elapsed, observed_active, lock_free)
}

fn cooperative() -> (Duration, usize, bool) {
    let cancel = Arc::new(AtomicBool::new(false));
    let active = Arc::new(AtomicUsize::new(0));
    let provider = Arc::new(Mutex::new(()));
    cancel_later(Arc::clone(&cancel));
    let started = Instant::now();
    {
        let _guard = provider.lock().expect("provider lock");
        active.fetch_add(1, Ordering::AcqRel);
        while !cancel.load(Ordering::Acquire) { thread::sleep(Duration::from_millis(2)); }
        active.fetch_sub(1, Ordering::AcqRel);
    }
    let elapsed = started.elapsed();
    let observed_active = active.load(Ordering::Acquire);
    let lock_free = provider.try_lock().is_ok();
    (elapsed, observed_active, lock_free)
}
