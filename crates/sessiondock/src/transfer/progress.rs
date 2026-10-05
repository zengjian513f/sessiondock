//! Ephemeral work counters. No journal writes, timers, or background scanning.
use serde_json::{Value, json};
use std::{
    cell::RefCell,
    collections::BTreeMap,
    io,
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicU64, Ordering},
    },
};

#[derive(Default)]
pub struct Registry(Mutex<BTreeMap<String, Weak<Work>>>);
impl Registry {
    pub fn start(&self, id: &str) -> Arc<Work> {
        let mut entries = self.0.lock().unwrap_or_else(|e| e.into_inner());
        entries.retain(|_, entry| entry.strong_count() > 0);
        let work = entries.get(id).and_then(Weak::upgrade).unwrap_or_default();
        entries.insert(id.into(), Arc::downgrade(&work));
        work
    }
    pub fn snapshot(&self, id: &str) -> Value {
        let work = self
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(id)
            .and_then(Weak::upgrade);
        work.map_or_else(|| json!([]), |work| work.snapshot())
    }
    pub fn enter(&self, id: &str) -> Scope {
        Scope::enter(self.start(id))
    }
}
#[derive(Default)]
pub struct Work(Mutex<Vec<Arc<Counter>>>);
struct Counter {
    label: String,
    unit: &'static str,
    total: Option<u64>,
    done: AtomicU64,
}
impl Work {
    pub fn task(self: &Arc<Self>, label: &str, unit: &'static str, total: Option<u64>) -> Task {
        let counter = Arc::new(Counter {
            label: label.into(),
            unit,
            total,
            done: AtomicU64::new(0),
        });
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(counter.clone());
        Task(Some((self.clone(), counter)))
    }
    pub fn snapshot(&self) -> Value {
        json!(self.0.lock().unwrap_or_else(|e| e.into_inner()).iter().map(|c| json!({
            "label":c.label,"unit":c.unit,"total":c.total,"done":c.done.load(Ordering::Relaxed)
        })).collect::<Vec<_>>())
    }
}
thread_local! { static CURRENT: RefCell<Option<Arc<Work>>> = const { RefCell::new(None) }; }
pub struct Scope(Option<Arc<Work>>);
impl Scope {
    fn enter(work: Arc<Work>) -> Self {
        Self(CURRENT.with(|slot| slot.replace(Some(work))))
    }
}
impl Drop for Scope {
    fn drop(&mut self) {
        CURRENT.with(|slot| slot.replace(self.0.take()));
    }
}
pub struct Task(Option<(Arc<Work>, Arc<Counter>)>);
impl Task {
    pub fn new(label: &str, unit: &'static str, total: Option<u64>) -> Self {
        CURRENT.with(|slot| {
            slot.borrow()
                .as_ref()
                .map_or(Self(None), |work| work.task(label, unit, total))
        })
    }
    pub fn set(&self, done: u64) {
        if let Some((_, c)) = &self.0 {
            c.done.store(done, Ordering::Relaxed);
        }
    }
    pub fn add(&self, done: u64) {
        if let Some((_, c)) = &self.0 {
            c.done.fetch_add(done, Ordering::Relaxed);
        }
    }
}
impl Drop for Task {
    fn drop(&mut self) {
        if let Some((work, counter)) = &self.0 {
            work.0
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .retain(|c| !Arc::ptr_eq(c, counter));
        }
    }
}
/// Count buffered I/O, not every tiny serde write. Use inside BufReader/Writer.
pub struct Io<T>(pub T, pub Task);
impl<T: io::Read> io::Read for Io<T> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        let n = self.0.read(bytes)?;
        self.1.add(n as u64);
        Ok(n)
    }
}
impl<T: io::Write> io::Write for Io<T> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        // Serde may hand us a whole multi-megabyte string in one write.
        // Return short writes so write_all publishes real progress per 64 KiB.
        let n = self.0.write(&bytes[..bytes.len().min(65536)])?;
        self.1.add(n as u64);
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}
