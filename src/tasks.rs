//! Background work: jobs run on a small worker pool; their results come
//! back as closures run on the UI thread with `&mut T` (`Tasks::process`).
//! The `wake` callback tells the event loop that results are waiting.

use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;

pub type Done<T> = Box<dyn FnOnce(&mut T) + Send>;
type Job<T> = Box<dyn FnOnce() -> Done<T> + Send>;

pub type Waker = Arc<dyn Fn() + Send + Sync>;

pub struct Tasks<T> {
    queue: Sender<Job<T>>,
    inbox: Arc<Mutex<Vec<Done<T>>>>,
    wake: Waker,
}

impl<T> Clone for Tasks<T> {
    fn clone(&self) -> Self {
        Tasks { queue: self.queue.clone(), inbox: self.inbox.clone(), wake: self.wake.clone() }
    }
}

impl<T: 'static> Tasks<T> {
    pub fn new(threads: usize, wake: Waker) -> Self {
        let (tx, rx) = mpsc::channel::<Job<T>>();
        let rx = Arc::new(Mutex::new(rx));
        let inbox: Arc<Mutex<Vec<Done<T>>>> = Arc::default();
        for i in 0..threads.max(1) {
            let rx: Arc<Mutex<Receiver<Job<T>>>> = rx.clone();
            let inbox = inbox.clone();
            let wake = wake.clone();
            let _ = thread::Builder::new().name(format!("worker-{i}")).spawn(move || {
                loop {
                    let job = { rx.lock().unwrap().recv() };
                    let Ok(job) = job else { return };
                    // A panicking job only loses its own result.
                    if let Ok(done) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job)) {
                        inbox.lock().unwrap().push(done);
                        wake();
                    }
                }
            });
        }
        Tasks { queue: tx, inbox, wake }
    }

    /// Run `job` on a worker, then `done(&mut T, result)` on the UI thread.
    pub fn spawn<R: Send + 'static>(
        &self,
        job: impl FnOnce() -> R + Send + 'static,
        done: impl FnOnce(&mut T, R) + Send + 'static,
    ) {
        let _ = self.queue.send(Box::new(move || {
            let r = job();
            Box::new(move |t: &mut T| done(t, r)) as Done<T>
        }));
    }

    /// Run `f` on the UI thread soon (from any thread).
    pub fn post(&self, f: impl FnOnce(&mut T) + Send + 'static) {
        self.inbox.lock().unwrap().push(Box::new(f));
        (self.wake)();
    }

    pub fn wake(&self) {
        (self.wake)();
    }

    pub fn waker(&self) -> Waker {
        self.wake.clone()
    }

    /// The results that arrived; call `process` on the UI thread.
    pub fn take(&self) -> Vec<Done<T>> {
        std::mem::take(&mut *self.inbox.lock().unwrap())
    }

    pub fn pending(&self) -> bool {
        !self.inbox.lock().unwrap().is_empty()
    }
}
