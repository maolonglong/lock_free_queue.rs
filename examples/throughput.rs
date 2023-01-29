use lock_free_queue::LockFreeQueue;
use queuecheck::queuecheck_bench_throughput;
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

fn main() {
    let ops: f64 = (0..100)
        .map(|_| {
            let queue = Arc::new(LockFreeQueue::new());
            queuecheck_bench_throughput!(
                (1_000, 100_000),
                vec![queue.clone(), queue.clone()],
                vec![queue.clone(), queue],
                |p: &LockFreeQueue<i32>, i: i32| p.push(i),
                |c: &LockFreeQueue<i32>| c.pop()
            )
        })
        .sum::<f64>()
        / 100_f64;
    println!("lock-free: {ops:.3} operation/second");

    let ops: f64 = (0..100)
        .map(|_| {
            let deque = Arc::new(Mutex::new(VecDeque::new()));
            queuecheck_bench_throughput!(
                (1_000, 100_000),
                vec![deque.clone(), deque.clone()],
                vec![deque.clone(), deque],
                |p: &Mutex<VecDeque<i32>>, i: i32| p.lock().unwrap().push_back(i),
                |c: &Mutex<VecDeque<i32>>| c.lock().unwrap().pop_front()
            )
        })
        .sum::<f64>()
        / 100_f64;
    println!("mutex: {ops:.3} operation/second");
}
