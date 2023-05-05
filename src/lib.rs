use std::mem::MaybeUninit;
use std::sync::atomic::{AtomicUsize, Ordering};

use crossbeam::epoch::{self, Atomic, Owned, Shared};

type Link<T> = Atomic<Node<T>>;

struct Node<T> {
    elem: MaybeUninit<T>,
    next: Link<T>,
}

impl<T> Default for Node<T> {
    fn default() -> Self {
        Self::dummy()
    }
}

impl<T> Node<T> {
    fn new(elem: T) -> Self {
        Node {
            elem: MaybeUninit::new(elem),
            next: Atomic::null(),
        }
    }

    fn dummy() -> Self {
        Node {
            elem: MaybeUninit::uninit(),
            next: Atomic::null(),
        }
    }
}

pub struct LockFreeQueue<T> {
    head: Link<T>,
    tail: Link<T>,
    len: AtomicUsize,
}

unsafe impl<T: Send> Send for LockFreeQueue<T> {}
unsafe impl<T: Send> Sync for LockFreeQueue<T> {}

impl<T> Default for LockFreeQueue<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> LockFreeQueue<T> {
    pub fn new() -> Self {
        let head = Atomic::new(Node::dummy());
        let tail = head.clone();
        LockFreeQueue {
            head,
            tail,
            len: AtomicUsize::new(0),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn len(&self) -> usize {
        self.len.load(Ordering::SeqCst)
    }

    pub fn push(&self, elem: T) {
        let guard = &epoch::pin();
        let new_node = Owned::new(Node::new(elem)).into_shared(guard);
        loop {
            let tail = self.tail.load(Ordering::Acquire, guard);
            let tail_next_ref = unsafe { &(*tail.as_raw()).next };
            let tail_next_shared = tail_next_ref.load(Ordering::Acquire, guard);
            if tail == self.tail.load(Ordering::Acquire, guard) {
                if tail_next_shared.is_null() {
                    if tail_next_ref
                        .compare_exchange(
                            Shared::null(),
                            new_node,
                            Ordering::Release,
                            Ordering::Relaxed,
                            guard,
                        )
                        .is_ok()
                    {
                        let _ = self.tail.compare_exchange(
                            tail,
                            new_node,
                            Ordering::Release,
                            Ordering::Relaxed,
                            guard,
                        );
                        self.len.fetch_add(1, Ordering::SeqCst);
                        return;
                    }
                } else {
                    let _ = self.tail.compare_exchange(
                        tail,
                        tail_next_shared,
                        Ordering::Release,
                        Ordering::Relaxed,
                        guard,
                    );
                }
            }
        }
    }

    pub fn pop(&self) -> Option<T> {
        let guard = &epoch::pin();
        loop {
            let head = self.head.load(Ordering::Acquire, guard);
            let tail = self.tail.load(Ordering::Acquire, guard);
            let head_next = unsafe { (*head.as_raw()).next.load(Ordering::Acquire, guard) };
            if head == self.head.load(Ordering::Acquire, guard) {
                if head == tail {
                    if head_next.is_null() {
                        return None;
                    }
                    let _ = self.tail.compare_exchange(
                        tail,
                        head_next,
                        Ordering::Release,
                        Ordering::Relaxed,
                        guard,
                    );
                } else if self
                    .head
                    .compare_exchange(head, head_next, Ordering::Release, Ordering::Relaxed, guard)
                    .is_ok()
                {
                    let elem = unsafe {
                        guard.defer_destroy(head);
                        (*head_next.as_raw()).elem.assume_init_read()
                    };
                    let _ = self.len.fetch_sub(1, Ordering::SeqCst);
                    return Some(elem);
                }
            }
        }
    }
}

impl<T> Drop for LockFreeQueue<T> {
    fn drop(&mut self) {
        while self.pop().is_some() {}
        let guard = &epoch::pin();
        let h = self.head.load_consume(guard);
        unsafe {
            guard.defer_destroy(h);
        }
    }
}

/// Copied from: https://github.com/ClSlaid/l3queue/blob/466f507186cd342e8eb886e79d209b7606460b30/src/he_queue.rs#L166-L333
#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicI32;
    use std::sync::{Arc, Barrier};
    use std::thread;

    use super::*;

    #[test]
    fn test_single() {
        let q = LockFreeQueue::new();
        q.push(1);
        q.push(1);
        q.push(4);
        q.push(5);
        q.push(1);
        q.push(4);
        assert_eq!(q.pop(), Some(1));
        assert_eq!(q.pop(), Some(1));
        assert_eq!(q.pop(), Some(4));
        assert_eq!(q.pop(), Some(5));
        assert_eq!(q.pop(), Some(1));
        assert_eq!(q.pop(), Some(4));
    }

    #[test]
    fn test_concurrent_send() {
        let pad = 100000_u128;

        let p1 = Arc::new(LockFreeQueue::new());
        let p2 = p1.clone();
        let c = p1.clone();
        let ba1 = Arc::new(Barrier::new(3));
        let ba2 = ba1.clone();
        let ba3 = ba1.clone();
        let t1 = thread::spawn(move || {
            for i in 0..pad {
                p1.push(i);
            }
            ba1.wait();
        });
        let t2 = thread::spawn(move || {
            for i in pad..(2 * pad) {
                p2.push(i);
            }
            ba2.wait();
        });
        // receive after send is finished
        ba3.wait();
        let mut sum = 0;
        while let Some(got) = c.pop() {
            sum += got;
        }
        let _ = t1.join();
        let _ = t2.join();
        assert_eq!(sum, (0..(2 * pad)).sum())
    }

    #[test]
    fn test_mpsc() {
        let pad = 100_0000u128;

        let flag = Arc::new(AtomicI32::new(3));
        let flag1 = flag.clone();
        let flag2 = flag.clone();
        let flag3 = flag.clone();
        let p1 = Arc::new(LockFreeQueue::new());
        let p2 = p1.clone();
        let p3 = p1.clone();
        let c = p1.clone();

        let t1 = thread::spawn(move || {
            for i in 0..pad {
                p1.push(i);
            }
            flag1.fetch_sub(1, Ordering::SeqCst);
        });
        let t2 = thread::spawn(move || {
            for i in pad..(2 * pad) {
                p2.push(i);
            }
            flag2.fetch_sub(1, Ordering::SeqCst);
        });
        let t3 = thread::spawn(move || {
            for i in (2 * pad)..(3 * pad) {
                p3.push(i);
            }
            flag3.fetch_sub(1, Ordering::SeqCst);
        });

        let mut sum = 0;
        while flag.load(Ordering::SeqCst) != 0 || !c.is_empty() {
            if let Some(num) = c.pop() {
                sum += num;
            }
        }

        t1.join().unwrap();
        t2.join().unwrap();
        t3.join().unwrap();
        assert_eq!(sum, (0..(3 * pad)).sum());
    }

    #[test]
    fn test_mpmc() {
        let pad = 10_0000u128;

        let flag = Arc::new(AtomicI32::new(3));
        let flag_c = flag.clone();
        let flag1 = flag.clone();
        let flag2 = flag.clone();
        let flag3 = flag.clone();

        let p1 = Arc::new(LockFreeQueue::new());
        let p2 = p1.clone();
        let p3 = p1.clone();
        let c1 = p1.clone();
        let c2 = p1.clone();

        let producer1 = thread::spawn(move || {
            for i in 0..pad {
                p1.push(i);
            }
            flag1.fetch_sub(1, Ordering::SeqCst);
        });
        let producer2 = thread::spawn(move || {
            for i in pad..(2 * pad) {
                p2.push(i);
            }
            flag2.fetch_sub(1, Ordering::SeqCst);
        });
        let producer3 = thread::spawn(move || {
            for i in (2 * pad)..(3 * pad) {
                p3.push(i);
            }
            flag3.fetch_sub(1, Ordering::SeqCst);
        });

        let consumer = thread::spawn(move || {
            let mut sum = 0;
            while flag_c.load(Ordering::SeqCst) != 0 || !c2.is_empty() {
                if let Some(num) = c2.pop() {
                    sum += num;
                }
            }
            sum
        });

        let mut sum = 0;
        while flag.load(Ordering::SeqCst) != 0 || !c1.is_empty() {
            if let Some(num) = c1.pop() {
                sum += num;
            }
        }

        producer1.join().unwrap();
        producer2.join().unwrap();
        producer3.join().unwrap();

        let s = consumer.join().unwrap();
        sum += s;
        assert_eq!(sum, (0..(3 * pad)).sum());
    }
}
