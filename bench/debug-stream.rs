//! In-memory audit probe only. No driver, devices, IPC, UI, or output APIs.
use std::alloc::{GlobalAlloc, Layout, System};
use std::hint::black_box;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;

struct CountAllocator;
static COUNT: AtomicBool = AtomicBool::new(false);
static ALLOCS: AtomicU64 = AtomicU64::new(0);
static BYTES: AtomicU64 = AtomicU64::new(0);
#[global_allocator]
static ALLOCATOR: CountAllocator = CountAllocator;
unsafe impl GlobalAlloc for CountAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if COUNT.load(Ordering::Relaxed) {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
            BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
        }
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}
// Match the public tap packet bound; only the in-memory source module is linked.
pub const MAX_BYTES: usize = u16::MAX as usize;
#[path = "../crates/otd-core/src/debug/stream.rs"]
mod stream;
fn mix(hash: &mut u64, value: u64) {
    *hash = (*hash ^ value).wrapping_mul(0x100000001b3);
}
fn hash_batch(hash: &mut u64, sub: &mut stream::Subscription, limit: usize, bytes: usize) {
    let batch = sub.read(limit, bytes).unwrap();
    mix(hash, batch.lost);
    mix(hash, batch.packets.len() as u64);
    for p in batch.packets {
        for v in [p.sequence, p.gap_generation, p.session, p.auxiliary as u64, p.bytes.len() as u64] {
            mix(hash, v);
        }
        for b in p.bytes { mix(hash, b as u64); }
    }
}
fn corpus() -> u64 {
    let mut a = stream::subscribe().unwrap();
    let mut b = stream::subscribe().unwrap();
    let mut hash = 0xcbf29ce484222325;
    let data = [37u8; 65536];
    hash_batch(&mut hash, &mut a, 64, 65535);
    for i in 0..9000 {
        let length = if i % 29 == 0 { 65535 } else { (i * 31) % 512 };
        stream::record(i % 3 + 1, &data[..length as usize], i % 2 == 0);
        if i % 97 == 0 { stream::record(1, &data, false); } // Oversize loss boundary.
        if i % 11 == 0 { hash_batch(&mut hash, &mut a, (i % 64 + 1) as usize, 65535); }
        if i % 350 == 0 { hash_batch(&mut hash, &mut b, 64, 262144); }
    }
    for _ in 0..150 {
        hash_batch(&mut hash, &mut a, 64, 65535);
        hash_batch(&mut hash, &mut b, 31, 262144);
    }
    drop(a); drop(b);
    let mut sub = stream::subscribe().unwrap();
    for i in 0..8192 { stream::record(i % 2 + 1, &data[..8], i % 2 == 0); }
    for _ in 0..100 { hash_batch(&mut hash, &mut sub, 64, 65535); }
    hash
}
fn main() {
    println!("corpus_digest,{:016x}", corpus());
    println!("reports,median_ns_per_empty_read,allocations_per_read,allocated_bytes_per_read");
    for count in [0usize, 64, 512, 4096] {
        let mut sub = stream::subscribe().unwrap();
        for _ in 0..count { stream::record(1, &[42; 64], false); }
        loop { if sub.read(64, 65535).unwrap().packets.is_empty() { break; } }
        for _ in 0..100 { black_box(sub.read(64, 65535).unwrap()); }
        let mut samples = [0.0f64; 5];
        for sample in &mut samples {
            let start = Instant::now();
            for _ in 0..10_000 { black_box(sub.read(black_box(64), black_box(65535)).unwrap()); }
            *sample = start.elapsed().as_nanos() as f64 / 10_000.0;
        }
        samples.sort_by(f64::total_cmp);
        ALLOCS.store(0, Ordering::Relaxed); BYTES.store(0, Ordering::Relaxed);
        COUNT.store(true, Ordering::Relaxed);
        for _ in 0..100 { black_box(sub.read(64, 65535).unwrap()); }
        COUNT.store(false, Ordering::Relaxed);
        println!("{count},{:.1},{:.1},{:.1}", samples[2], ALLOCS.load(Ordering::Relaxed) as f64 / 100.0, BYTES.load(Ordering::Relaxed) as f64 / 100.0);
    }
}
