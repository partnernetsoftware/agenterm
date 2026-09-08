use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::thread;
use std::time::{Duration, Instant};

const WORK: Duration = Duration::from_millis(250);
const CANCEL_AT: Duration = Duration::from_millis(25);

fn main() {
    let (sync_elapsed, sync_active, sync_lock_free, sync_next_call_wait) = synchronous();
    let (detached_elapsed, detached_active, detached_lock_free, next_call_wait) = detached();
    let (
        cooperative_elapsed,
        cooperative_active,
        cooperative_lock_free,
        cooperative_next_call_wait,
    ) = cooperative();
    println!(
        "synchronous_ms={} synchronous_active={} synchronous_lock_free={} synchronous_next_call_wait_ms={} detached_ms={} detached_active={} detached_lock_free={} detached_next_call_wait_ms={} cooperative_ms={} cooperative_active={} cooperative_lock_free={} cooperative_next_call_wait_ms={}",
        sync_elapsed.as_millis(),
        sync_active,
        sync_lock_free,
        sync_next_call_wait.as_millis(),
        detached_elapsed.as_millis(),
        detached_active,
        detached_lock_free,
        next_call_wait.as_millis(),
        cooperative_elapsed.as_millis(),
        cooperative_active,
        cooperative_lock_free,
        cooperative_next_call_wait.as_millis()
    );
    assert!(sync_elapsed >= Duration::from_millis(200));
    assert_eq!(sync_active, 0);
    assert!(sync_lock_free);
    assert!(sync_next_call_wait < Duration::from_millis(50));
    assert!(detached_elapsed < Duration::from_millis(100));
    assert_eq!(detached_active, 1);
    assert!(!detached_lock_free);
    assert!(next_call_wait >= Duration::from_millis(150));
    assert!(cooperative_elapsed < Duration::from_millis(100));
    assert_eq!(cooperative_active, 0);
    assert!(cooperative_lock_free);
    assert!(cooperative_next_call_wait < Duration::from_millis(50));
}
fn cancel_later(flag: Arc<AtomicBool>) {
    thread::spawn(move || {
        thread::sleep(CANCEL_AT);
        flag.store(true, Ordering::Release);
    });
}

fn synchronous() -> (Duration, usize, bool, Duration) {
    let cancel = Arc::new(AtomicBool::new(false));
    let active = AtomicUsize::new(0);
    let provider = Mutex::new(());
    cancel_later(Arc::clone(&cancel));
    let started = Instant::now();
    {
        let _guard = provider.lock().expect("provider lock");
        active.fetch_add(1, Ordering::AcqRel);
        thread::sleep(WORK);
        active.fetch_sub(1, Ordering::AcqRel);
    }
    assert!(cancel.load(Ordering::Acquire));
    let elapsed = started.elapsed();
    let observed_active = active.load(Ordering::Acquire);
    let lock_free = provider.try_lock().is_ok();
    let second_started = Instant::now();
    let second_guard = provider.lock().expect("second provider call");
    let next_call_wait = second_started.elapsed();
    drop(second_guard);
    (elapsed, observed_active, lock_free, next_call_wait)
}

fn detached() -> (Duration, usize, bool, Duration) {
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
    while active.load(Ordering::Acquire) == 0 {
        thread::yield_now();
    }
    let started = Instant::now();
    while !cancel.load(Ordering::Acquire) {
        thread::sleep(Duration::from_millis(2));
    }
    let elapsed = started.elapsed();
    let observed_active = active.load(Ordering::Acquire);
    let lock_free = provider.try_lock().is_ok();
    let second_started = Instant::now();
    let second_guard = provider.lock().expect("second provider call");
    let next_call_wait = second_started.elapsed();
    drop(second_guard);
    handle.join().expect("detached probe worker");
    (elapsed, observed_active, lock_free, next_call_wait)
}

fn cooperative() -> (Duration, usize, bool, Duration) {
    let cancel = Arc::new(AtomicBool::new(false));
    let active = Arc::new(AtomicUsize::new(0));
    let provider = Arc::new(Mutex::new(()));
    cancel_later(Arc::clone(&cancel));
    let started = Instant::now();
    {
        let _guard = provider.lock().expect("provider lock");
        active.fetch_add(1, Ordering::AcqRel);
        while !cancel.load(Ordering::Acquire) {
            thread::sleep(Duration::from_millis(2));
        }
        active.fetch_sub(1, Ordering::AcqRel);
    }
    let elapsed = started.elapsed();
    let observed_active = active.load(Ordering::Acquire);
    let lock_free = provider.try_lock().is_ok();
    let second_started = Instant::now();
    let second_guard = provider.lock().expect("second provider call");
    let next_call_wait = second_started.elapsed();
    drop(second_guard);
    (elapsed, observed_active, lock_free, next_call_wait)
}
