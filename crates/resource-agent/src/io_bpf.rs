//! Runtime libbpf loading; the object is compiled at build time and embedded.
use super::{IoKind, IoSample};
use process_links::Process;
use std::{
    ffi::{CStr, c_char, c_int, c_long, c_void},
    io,
};
type P = *mut c_void;
macro_rules! function {
    ($lib:expr,$name:literal,$ty:ty) => {{
        let p = libc::dlsym($lib, concat!($name, "\0").as_ptr().cast());
        if p.is_null() {
            return Err(io::Error::other(concat!("missing libbpf symbol ", $name)));
        }
        std::mem::transmute::<P, $ty>(p)
    }};
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Key {
    start_ns: u64,
    device: u64,
    generation: u64,
    pid: u32,
    kind: u32,
}
#[repr(C)]
#[derive(Default)]
struct Counter {
    bytes: u64,
    operations: u64,
}
#[repr(C)]
struct Config {
    start_ns: u64,
    stop_ns: u64,
    uid: u32,
    padding: u32,
}
struct Api {
    lib: P,
    close: unsafe extern "C" fn(P),
    destroy: unsafe extern "C" fn(P) -> c_int,
    lookup: unsafe extern "C" fn(c_int, *const c_void, P) -> c_int,
    next: unsafe extern "C" fn(c_int, *const c_void, P) -> c_int,
    delete: unsafe extern "C" fn(c_int, *const c_void) -> c_int,
}
pub struct Probe {
    api: Api,
    object: P,
    links: Vec<P>,
    totals: c_int,
    losses: c_int,
    pub start_ns: u64,
    pub stop_ns: u64,
}
pub fn monotonic_ns() -> u64 {
    let mut ts: libc::timespec = unsafe { std::mem::zeroed() };
    unsafe {
        libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts);
    }
    ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64
}
impl Probe {
    pub fn open(uid: u32, deadline: u64) -> io::Result<Self> {
        unsafe {
            let lib = libc::dlopen(c"libbpf.so.1".as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL);
            if lib.is_null() {
                return Err(io::Error::other("libbpf.so.1 unavailable"));
            }
            // Loading failures terminate the short-lived helper, which also closes
            // the dynamic library; object/link cleanup below covers partial attach.
            let open = function!(
                lib,
                "bpf_object__open_mem",
                unsafe extern "C" fn(*const c_void, usize, P) -> P
            );
            let close = function!(lib, "bpf_object__close", unsafe extern "C" fn(P));
            let destroy = function!(lib, "bpf_link__destroy", unsafe extern "C" fn(P) -> c_int);
            let error = function!(lib, "libbpf_get_error", unsafe extern "C" fn(P) -> c_long);
            let load = function!(lib, "bpf_object__load", unsafe extern "C" fn(P) -> c_int);
            let find = function!(
                lib,
                "bpf_object__find_map_by_name",
                unsafe extern "C" fn(P, *const c_char) -> P
            );
            let next_map = function!(lib, "bpf_object__next_map", unsafe extern "C" fn(P, P) -> P);
            let name = function!(
                lib,
                "bpf_map__name",
                unsafe extern "C" fn(P) -> *const c_char
            );
            let update = function!(
                lib,
                "bpf_map_update_elem",
                unsafe extern "C" fn(c_int, *const c_void, *const c_void, u64) -> c_int
            );
            let fd = function!(lib, "bpf_map__fd", unsafe extern "C" fn(P) -> c_int);
            let next_program = function!(
                lib,
                "bpf_object__next_program",
                unsafe extern "C" fn(P, P) -> P
            );
            let attach = function!(lib, "bpf_program__attach", unsafe extern "C" fn(P) -> P);
            let api = Api {
                lib,
                close,
                destroy,
                lookup: function!(
                    lib,
                    "bpf_map_lookup_elem",
                    unsafe extern "C" fn(c_int, *const c_void, P) -> c_int
                ),
                next: function!(
                    lib,
                    "bpf_map_get_next_key",
                    unsafe extern "C" fn(c_int, *const c_void, P) -> c_int
                ),
                delete: function!(
                    lib,
                    "bpf_map_delete_elem",
                    unsafe extern "C" fn(c_int, *const c_void) -> c_int
                ),
            };
            let bytes = include_bytes!(concat!(env!("OUT_DIR"), "/io.bpf.o"));
            let object = open(bytes.as_ptr().cast(), bytes.len(), std::ptr::null_mut());
            if object.is_null() || error(object) != 0 {
                libc::dlclose(lib);
                return Err(io::Error::other("BPF object open failed"));
            }
            let start_ns = monotonic_ns();
            let mut probe = Self {
                api,
                object,
                links: Vec::new(),
                totals: -1,
                losses: -1,
                start_ns,
                stop_ns: deadline,
            };

            let mut map = std::ptr::null_mut();
            let mut config_map = std::ptr::null_mut();
            loop {
                map = next_map(object, map);
                if map.is_null() {
                    break;
                }
                if CStr::from_ptr(name(map)).to_bytes().ends_with(b".bss") {
                    config_map = map;
                }
            }
            if config_map.is_null() || load(object) != 0 {
                return Err(io::Error::other("BPF verifier/kernel incompatibility"));
            }
            probe.totals = fd(find(object, c"totals".as_ptr()));
            probe.losses = fd(find(object, c"losses".as_ptr()));
            if probe.totals < 0 || probe.losses < 0 {
                return Err(io::Error::other("BPF maps unavailable"));
            }
            let mut program = std::ptr::null_mut();
            loop {
                program = next_program(object, program);
                if program.is_null() {
                    break;
                }
                let link = attach(program);
                if link.is_null() || error(link) != 0 {
                    return Err(io::Error::other("BPF attach/kernel incompatibility"));
                }
                probe.links.push(link);
            }
            // Probes stay disabled until every link is attached. Start the
            // first full two-second generation at this activation boundary.
            probe.start_ns = monotonic_ns();
            let config = Config {
                start_ns: probe.start_ns,
                stop_ns: deadline,
                uid,
                padding: 0,
            };
            let zero = 0u32;
            if update(
                fd(config_map),
                (&zero as *const u32).cast(),
                (&config as *const Config).cast(),
                0,
            ) != 0
            {
                return Err(io::Error::other("BPF activation failed"));
            }
            Ok(probe)
        }
    }
    pub fn losses(&self) -> io::Result<u64> {
        let key = 0u32;
        let mut value = 0u64;
        if unsafe {
            (self.api.lookup)(
                self.losses,
                (&key as *const u32).cast(),
                (&mut value as *mut u64).cast(),
            )
        } != 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(value)
    }
    /// Future generations remain in the map. A full scan is bounded by the map
    /// capacity; late writes into already completed generations count as loss.
    pub fn drain(&self, generation: u64) -> io::Result<(Vec<IoSample>, u64)> {
        let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) }.max(1) as u64;
        let mut keys = Vec::new();
        let mut previous: Option<Key> = None;
        for _ in 0..32769 {
            let mut next = Key::default();
            let ptr = previous
                .as_ref()
                .map_or(std::ptr::null(), |k| (k as *const Key).cast());
            if unsafe { (self.api.next)(self.totals, ptr, (&mut next as *mut Key).cast()) } != 0 {
                if io::Error::last_os_error().raw_os_error() != Some(libc::ENOENT) {
                    return Err(io::Error::last_os_error());
                }
                break;
            }
            previous = Some(next);
            if next.generation <= generation {
                keys.push(next);
            }
            if keys.len() > 32768 {
                return Err(io::Error::other("BPF map scan overflow"));
            }
        }
        let mut samples = Vec::with_capacity(keys.len());
        let mut lost = 0;
        for key in keys {
            let mut counter = Counter::default();
            if unsafe {
                (self.api.lookup)(
                    self.totals,
                    (&key as *const Key).cast(),
                    (&mut counter as *mut Counter).cast(),
                )
            } != 0
            {
                lost += 1;
                continue;
            }
            if unsafe { (self.api.delete)(self.totals, (&key as *const Key).cast()) } != 0 {
                lost += 1;
                continue;
            }
            if key.generation < generation {
                lost += 1;
                continue;
            }
            let kind = match key.kind {
                0 => IoKind::TcpSend,
                1 => IoKind::TcpReceive,
                2 => IoKind::LocalRead,
                3 => IoKind::LocalWrite,
                4 => IoKind::NfsRead,
                5 => IoKind::NfsWrite,
                _ => {
                    lost += 1;
                    continue;
                }
            };
            samples.push(IoSample {
                process: Process {
                    pid: key.pid,
                    start: (key.start_ns as u128 * ticks as u128 / 1_000_000_000) as u64,
                },
                device: key.device,
                kind,
                bytes: counter.bytes,
                operations: counter.operations,
                generation,
            });
        }
        Ok((samples, lost))
    }
}
impl Drop for Probe {
    fn drop(&mut self) {
        unsafe {
            for link in self.links.drain(..) {
                (self.api.destroy)(link);
            }
            (self.api.close)(self.object);
            libc::dlclose(self.api.lib);
        }
    }
}
